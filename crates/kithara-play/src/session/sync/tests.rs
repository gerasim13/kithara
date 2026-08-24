use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentSource, BeatMap, BeatMapId, BeatMapRevision, HostBeatMap, HostEpoch, LoadGeneration,
    SessionAnchor, SessionBeat, SessionFrame, SyncAdmission, SyncCapability, SyncError, SyncGroup,
    SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember, SyncMemberKind,
    SyncOperation, SyncOperationId, SyncStatusSnapshot, TopologyOperation, TopologyStamp,
    TransportOperation, TransportRevision,
};
use kithara_bufpool::PcmPool;
use kithara_events::EventBus;
use kithara_test_utils::kithara;

use super::Host;
use crate::session::state::Deck;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: test map identity space is available")
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero")
}

fn deck_with_player(id: BeatMapId, player_id: u64) -> Deck {
    Deck::new(
        player_id,
        id,
        EventBus::default(),
        Vec::new(),
        PcmPool::default(),
        sample_rate(),
    )
}

fn deck(id: BeatMapId) -> Deck {
    deck_with_player(id, 1)
}

fn track() -> HostBeatMap {
    track_with_id(map_id())
}

fn track_with_id(id: BeatMapId) -> HostBeatMap {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: track beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: track relation is valid");
    HostBeatMap::new(
        id,
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        None,
    )
}

fn map_member(map: HostBeatMap) -> (BeatMapId, SyncMember<Deck>) {
    let snapshot = map.snapshot();
    let id = snapshot.id();
    (
        id,
        SyncMember::Map {
            alignment: None,
            map: Box::new(map),
        },
    )
}

fn changed_topology(admission: SyncAdmission) -> TopologyStamp {
    match admission {
        SyncAdmission::TopologyChanged { topology, .. } => topology,
        _ => panic!("expected an admitted topology mutation"),
    }
}

fn topology(group: &dyn SyncGroup<NestedGroup = Deck>) -> SyncGroupSnapshot {
    group
        .topology()
        .expect("the production owner keeps a valid recursive topology")
}

#[kithara::test]
fn topology_batch_publishes_one_revision_with_only_the_replacement() {
    let deck_id = map_id();
    let mut deck = deck(deck_id);
    let group: &mut dyn SyncGroup<NestedGroup = Deck> = &mut deck;
    let before = topology(group);
    let (a_id, a) = map_member(track());
    let (b_id, b) = map_member(track());
    let (c_id, c) = map_member(track());

    let admitted = group
        .transact(SyncOperation::Topology {
            base: before.stamp(),
            operations: Box::new([
                TopologyOperation::Attach { member: a },
                TopologyOperation::Attach { member: b },
                TopologyOperation::Detach { member: a_id },
                TopologyOperation::Replace {
                    member: b_id,
                    replacement: c,
                },
            ]),
        })
        .expect("one valid batch is admitted atomically");
    let after = topology(group);

    assert_eq!(changed_topology(admitted), after.stamp());
    assert_eq!(after.stamp().group_id(), deck_id);
    assert_eq!(after.group_map(), before.group_map());
    assert_eq!(
        after.stamp().revision(),
        before
            .stamp()
            .revision()
            .checked_next()
            .expect("invariant: fixture topology revision can advance"),
    );
    assert_eq!(after.members().len(), 1);
    assert_eq!(after.members()[0].map().id(), c_id);
    assert!(after.members()[0].group_topology().is_none());
}

#[kithara::test]
fn stale_topology_base_leaves_the_published_snapshot_unchanged() {
    let deck_id = map_id();
    let mut deck = deck(deck_id);
    let group: &mut dyn SyncGroup<NestedGroup = Deck> = &mut deck;
    let initial = topology(group);
    let (_, first) = map_member(track());
    changed_topology(
        group
            .transact(SyncOperation::Topology {
                base: initial.stamp(),
                operations: Box::new([TopologyOperation::Attach { member: first }]),
            })
            .expect("invariant: fixture setup advances the topology"),
    );
    let before_rejection = topology(group);
    let (stale_member_id, stale_member) = map_member(track());

    let rejection = group
        .transact(SyncOperation::Topology {
            base: initial.stamp(),
            operations: Box::new([TopologyOperation::Attach {
                member: stale_member,
            }]),
        })
        .expect_err("an old topology base must be rejected before mutation");
    let (error, operation): (SyncError, SyncOperation<Deck>) = rejection.into();

    assert_eq!(
        error,
        SyncError::StaleTopology {
            expected: before_rejection.stamp(),
            given: initial.stamp(),
        }
    );
    let SyncOperation::Topology { operations, .. } = operation else {
        panic!("expected the rejected topology transaction");
    };
    let [TopologyOperation::Attach { member }] = operations.as_ref() else {
        panic!("expected the rejected member to remain owned by the caller");
    };
    assert_eq!(member.id(), stale_member_id);
    assert_eq!(topology(group), before_rejection);
}

#[kithara::test]
fn unknown_member_detach_leaves_the_published_snapshot_unchanged() {
    let deck_id = map_id();
    let mut deck = deck(deck_id);
    let group: &mut dyn SyncGroup<NestedGroup = Deck> = &mut deck;
    let before = topology(group);
    let unknown = map_id();

    let error = group
        .transact(SyncOperation::Topology {
            base: before.stamp(),
            operations: Box::new([TopologyOperation::Detach { member: unknown }]),
        })
        .expect_err("an unknown direct member cannot be detached");

    assert_eq!(
        error.error(),
        &SyncError::MemberNotFound {
            group_id: deck_id,
            member_id: unknown,
        }
    );
    assert_eq!(topology(group), before);
}

#[kithara::test]
fn host_rejects_map_members_without_advancing_its_operation() {
    let host_id = map_id();
    let mut host = Host::new(host_id, sample_rate());
    let before_attach = topology(&host);
    let (track_id, map) = map_member(track());

    let attach_error = host
        .transact(SyncOperation::Topology {
            base: before_attach.stamp(),
            operations: Box::new([TopologyOperation::Attach { member: map }]),
        })
        .expect_err("a host cannot own a track map directly");
    assert_eq!(
        attach_error.error(),
        &SyncError::InvalidMemberKind {
            group_id: host_id,
            member_id: track_id,
            expected: SyncMemberKind::Group,
            given: SyncMemberKind::Map,
        }
    );
    assert_eq!(topology(&host), before_attach);

    let first_deck_id = map_id();
    let first_admission = host
        .transact(SyncOperation::Topology {
            base: before_attach.stamp(),
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck(first_deck_id)),
                },
            }]),
        })
        .expect("a valid deck remains the first admitted operation");
    let SyncAdmission::TopologyChanged {
        operation: first_operation,
        ..
    } = first_admission
    else {
        panic!("expected a topology change");
    };
    assert_eq!(first_operation, SyncOperationId::first());

    let before_replace = topology(&host);
    let (replacement_id, replacement) = map_member(track());
    let replace_error = host
        .transact(SyncOperation::Topology {
            base: before_replace.stamp(),
            operations: Box::new([TopologyOperation::Replace {
                member: first_deck_id,
                replacement,
            }]),
        })
        .expect_err("a host cannot replace a deck with a track map");
    assert_eq!(
        replace_error.error(),
        &SyncError::InvalidMemberKind {
            group_id: host_id,
            member_id: replacement_id,
            expected: SyncMemberKind::Group,
            given: SyncMemberKind::Map,
        }
    );
    assert_eq!(topology(&host), before_replace);

    let second_deck_id = map_id();
    let second_admission = host
        .transact(SyncOperation::Topology {
            base: before_replace.stamp(),
            operations: Box::new([TopologyOperation::Replace {
                member: first_deck_id,
                replacement: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck(second_deck_id)),
                },
            }]),
        })
        .expect("a valid replacement remains the second admitted operation");
    let SyncAdmission::TopologyChanged {
        operation: second_operation,
        ..
    } = second_admission
    else {
        panic!("expected a topology change");
    };
    assert_eq!(
        second_operation,
        SyncOperationId::first()
            .checked_next()
            .expect("the fixture operation can advance")
    );
}

#[kithara::test]
fn deck_rejects_group_members_without_advancing_its_operation() {
    let deck_id = map_id();
    let mut deck = deck(deck_id);
    let before_attach = topology(&deck);
    let nested_id = map_id();

    let attach_error = deck
        .transact(SyncOperation::Topology {
            base: before_attach.stamp(),
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck_with_player(nested_id, 2)),
                },
            }]),
        })
        .expect_err("a deck cannot own another deck");
    assert_eq!(
        attach_error.error(),
        &SyncError::InvalidMemberKind {
            group_id: deck_id,
            member_id: nested_id,
            expected: SyncMemberKind::Map,
            given: SyncMemberKind::Group,
        }
    );
    assert_eq!(topology(&deck), before_attach);

    let (first_track_id, first_track) = map_member(track());
    let first_admission = deck
        .transact(SyncOperation::Topology {
            base: before_attach.stamp(),
            operations: Box::new([TopologyOperation::Attach {
                member: first_track,
            }]),
        })
        .expect("a valid track remains the first admitted operation");
    let SyncAdmission::TopologyChanged {
        operation: first_operation,
        ..
    } = first_admission
    else {
        panic!("expected a topology change");
    };
    assert_eq!(first_operation, SyncOperationId::first());

    let before_replace = topology(&deck);
    let replacement_id = map_id();
    let replace_error = deck
        .transact(SyncOperation::Topology {
            base: before_replace.stamp(),
            operations: Box::new([TopologyOperation::Replace {
                member: first_track_id,
                replacement: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck_with_player(replacement_id, 3)),
                },
            }]),
        })
        .expect_err("a deck cannot replace a track with another deck");
    assert_eq!(
        replace_error.error(),
        &SyncError::InvalidMemberKind {
            group_id: deck_id,
            member_id: replacement_id,
            expected: SyncMemberKind::Map,
            given: SyncMemberKind::Group,
        }
    );
    assert_eq!(topology(&deck), before_replace);

    let (_, second_track) = map_member(track());
    let second_admission = deck
        .transact(SyncOperation::Topology {
            base: before_replace.stamp(),
            operations: Box::new([TopologyOperation::Replace {
                member: first_track_id,
                replacement: second_track,
            }]),
        })
        .expect("a valid replacement remains the second admitted operation");
    let SyncAdmission::TopologyChanged {
        operation: second_operation,
        ..
    } = second_admission
    else {
        panic!("expected a topology change");
    };
    assert_eq!(
        second_operation,
        SyncOperationId::first()
            .checked_next()
            .expect("the fixture operation can advance")
    );
}

#[kithara::test]
fn host_atomically_attaches_a_deck() {
    let host_id = map_id();
    let deck_id = map_id();
    let mut host = Host::new(host_id, sample_rate());
    let mut deck = deck(deck_id);
    let deck_topology = {
        let group: &mut dyn SyncGroup<NestedGroup = Deck> = &mut deck;
        topology(group)
    };
    let group: &mut dyn SyncGroup<NestedGroup = Deck> = &mut host;
    let before = topology(group);
    let member = SyncMember::Group {
        alignment: None,
        group: Box::new(deck),
    };

    let admitted = group
        .transact(SyncOperation::Topology {
            base: before.stamp(),
            operations: Box::new([TopologyOperation::Attach { member }]),
        })
        .expect("a host admits one owned deck atomically");
    let after = topology(group);

    assert_eq!(changed_topology(admitted), after.stamp());
    assert_eq!(after.stamp().group_id(), host_id);
    assert_eq!(after.group_map(), before.group_map());
    assert_eq!(
        after.stamp().revision(),
        before
            .stamp()
            .revision()
            .checked_next()
            .expect("invariant: fixture topology revision can advance"),
    );
    assert_eq!(after.members().len(), 1);
    assert_eq!(after.members()[0].map().id(), deck_id);
    assert_eq!(
        after.members()[0]
            .group_topology()
            .expect("the attached member remains a group")
            .stamp(),
        deck_topology.stamp(),
    );
}

#[kithara::test]
fn nested_topology_change_advances_the_deck_and_host_once() {
    let host_id = map_id();
    let deck_id = map_id();
    let mut host = Host::new(host_id, sample_rate());
    let deck = deck(deck_id);
    let deck_base = topology(&deck).stamp();
    let host_base = topology(&host).stamp();
    changed_topology(
        host.transact(SyncOperation::Topology {
            base: host_base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck),
                },
            }]),
        })
        .expect("the fixture deck attaches to the host"),
    );
    let before = topology(&host);
    let (track_id, member) = map_member(track());

    let admitted = host
        .transact(SyncOperation::Topology {
            base: deck_base,
            operations: Box::new([TopologyOperation::Attach { member }]),
        })
        .expect("the rooted owner admits one valid nested mutation");
    let after = topology(&host);
    let nested = after.members()[0]
        .group_topology()
        .expect("the host member remains a deck");

    assert_eq!(changed_topology(admitted).group_id(), deck_id);
    assert_eq!(
        after.stamp().revision(),
        before
            .stamp()
            .revision()
            .checked_next()
            .expect("the fixture host revision can advance")
    );
    assert_eq!(
        nested.stamp().revision(),
        deck_base
            .revision()
            .checked_next()
            .expect("the fixture deck revision can advance")
    );
    assert_eq!(nested.members()[0].map().id(), track_id);
}

#[kithara::test]
fn host_routes_transport_to_the_canonical_deck_without_changing_root_topology() {
    let host_id = map_id();
    let deck_id = map_id();
    let mut host = Host::new(host_id, sample_rate());
    let host_base = topology(&host).stamp();
    changed_topology(
        host.transact(SyncOperation::Topology {
            base: host_base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck(deck_id)),
                },
            }]),
        })
        .expect("the fixture deck attaches to the host"),
    );
    let before = topology(&host);
    let deck_topology = host
        .deck(0)
        .expect("the canonical deck is owned by the host")
        .topology()
        .expect("the deck topology is valid")
        .stamp();

    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let admission = host
        .transact(SyncOperation::Transport {
            target: deck_id,
            load,
            transport,
            operation: TransportOperation::Play,
        })
        .expect("the host routes transport through the resident deck");

    assert!(matches!(
        admission,
        SyncAdmission::Accepted {
            operation,
            topology,
            load: observed_load,
            transport: observed_transport,
        } if operation == SyncOperationId::first()
            && topology == deck_topology
            && observed_load == load
            && observed_transport == transport
    ));
    assert_eq!(topology(&host), before);
    assert_eq!(
        host.status(),
        SyncStatusSnapshot::Off {
            topology: before.stamp()
        }
    );
    assert!(matches!(
        host.deck(0)
            .expect("the canonical deck remains owned by the host")
            .status(),
        SyncStatusSnapshot::Off { topology } if topology == deck_topology
    ));
}

#[kithara::test]
fn unknown_routed_target_leaves_root_and_operation_counters_unchanged() {
    let host_id = map_id();
    let deck_id = map_id();
    let mut host = Host::new(host_id, sample_rate());
    let host_base = topology(&host).stamp();
    changed_topology(
        host.transact(SyncOperation::Topology {
            base: host_base,
            operations: Box::new([TopologyOperation::Attach {
                member: SyncMember::Group {
                    alignment: None,
                    group: Box::new(deck(deck_id)),
                },
            }]),
        })
        .expect("the fixture deck attaches to the host"),
    );
    let before = topology(&host);
    let unknown = map_id();

    let error = host
        .transact(SyncOperation::Transport {
            target: unknown,
            load: LoadGeneration::first(),
            transport: TransportRevision::first(),
            operation: TransportOperation::Play,
        })
        .expect_err("an unknown routed target cannot consume an operation");

    assert_eq!(
        error.error(),
        &SyncError::GroupNotFound { group_id: unknown }
    );
    assert_eq!(topology(&host), before);
    let admission = host
        .transact(SyncOperation::Transport {
            target: deck_id,
            load: LoadGeneration::first(),
            transport: TransportRevision::first(),
            operation: TransportOperation::Play,
        })
        .expect("the first valid deck operation is still admitted");
    assert!(matches!(
        admission,
        SyncAdmission::Accepted {
            operation,
            ..
        } if operation == SyncOperationId::first()
    ));
    assert_eq!(topology(&host), before);
}

#[kithara::test]
fn duplicate_leaf_across_sibling_decks_leaves_the_root_unchanged() {
    let shared_id = map_id();
    let first_deck_id = map_id();
    let second_deck_id = map_id();
    let mut first_deck = deck_with_player(first_deck_id, 1);
    let first_base = topology(&first_deck).stamp();
    let (_, first_member) = map_member(track_with_id(shared_id));
    changed_topology(
        first_deck
            .transact(SyncOperation::Topology {
                base: first_base,
                operations: Box::new([TopologyOperation::Attach {
                    member: first_member,
                }]),
            })
            .expect("the first deck owns the shared fixture leaf"),
    );
    let second_deck = deck_with_player(second_deck_id, 2);
    let second_base = topology(&second_deck).stamp();
    let mut host = Host::new(map_id(), sample_rate());
    let host_base = topology(&host).stamp();
    changed_topology(
        host.transact(SyncOperation::Topology {
            base: host_base,
            operations: Box::new([
                TopologyOperation::Attach {
                    member: SyncMember::Group {
                        alignment: None,
                        group: Box::new(first_deck),
                    },
                },
                TopologyOperation::Attach {
                    member: SyncMember::Group {
                        alignment: None,
                        group: Box::new(second_deck),
                    },
                },
            ]),
        })
        .expect("two distinct fixture decks form a valid root"),
    );
    let before = topology(&host);
    let (_, duplicate) = map_member(track_with_id(shared_id));

    let error = host
        .transact(SyncOperation::Topology {
            base: second_base,
            operations: Box::new([TopologyOperation::Attach { member: duplicate }]),
        })
        .expect_err("one load identity cannot appear below sibling decks");

    assert_eq!(
        error.error(),
        &SyncError::Topology(SyncGroupTopologyError::DuplicateLeaf {
            member_id: shared_id,
        })
    );
    assert_eq!(topology(&host), before);

    let (_, unique) = map_member(track());
    let admission = host
        .transact(SyncOperation::Topology {
            base: second_base,
            operations: Box::new([TopologyOperation::Attach { member: unique }]),
        })
        .expect("the rejected mutation did not consume the deck operation");
    assert!(matches!(
        admission,
        SyncAdmission::TopologyChanged { operation, .. }
            if operation == SyncOperationId::first()
    ));
}

#[kithara::test]
fn same_stamp_with_different_map_content_is_not_idempotent() {
    let id = map_id();
    let mut host = Host::new(id, sample_rate());
    let current = host.snapshot();
    let conflicting = track_with_id(id).snapshot();

    let error = host
        .publish_map(conflicting)
        .expect_err("one immutable map stamp cannot name different geometry");

    assert_eq!(
        error,
        SyncError::StaleMapRevision {
            current: current.stamp(),
            given: current.stamp(),
        }
    );
    assert_eq!(host.snapshot(), current);
}

#[kithara::test]
fn disable_returns_an_unimplemented_deck_to_the_off_contract() {
    let deck_id = map_id();
    let mut deck = deck(deck_id);
    let topology = topology(&deck).stamp();
    let enable = deck
        .transact(SyncOperation::Sync {
            target: deck_id,
            load: LoadGeneration::first(),
            transport: TransportRevision::first(),
            source: AlignmentSource::Prepared,
            activation: SessionFrame::new(0),
            intent: SyncIntent::Enable,
        })
        .expect("the framework reports unavailable alignment without mutating audio");
    assert!(matches!(
        enable,
        SyncAdmission::Unavailable {
            topology: observed,
            capability: SyncCapability::Alignment,
            ..
        } if observed == topology
    ));
    assert!(matches!(
        deck.status(),
        SyncStatusSnapshot::Unavailable {
            topology: observed,
            capability: SyncCapability::Alignment,
            ..
        } if observed == topology
    ));

    let disable = deck
        .transact(SyncOperation::Sync {
            target: deck_id,
            load: LoadGeneration::first(),
            transport: TransportRevision::first(),
            source: AlignmentSource::Prepared,
            activation: SessionFrame::new(0),
            intent: SyncIntent::Disable,
        })
        .expect("disabled sync remains the identity contract");

    assert!(matches!(
        disable,
        SyncAdmission::Unchanged {
            topology: observed,
            ..
        } if observed == topology
    ));
    assert_eq!(deck.status(), SyncStatusSnapshot::Off { topology });
}
