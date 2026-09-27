use kithara_platform::sync::{Arc, atomic::Ordering};
use kithara_signal::{SessionFrame, TransportRevision};
use kithara_test_utils::kithara;
use kithara_warp::{BeatGridId, BeatGridRevision, BeatGridStamp, PresentationFrontier};

use super::*;
use crate::{
    LoadGeneration, SourceChange, SyncApplied, SyncExecutionStamp, SyncOperationId, SyncReceipt,
    SyncReceiptTx, TopologyRevision, TopologyStamp, sync_receipts,
};

fn stamp(member: BeatGridId) -> SyncExecutionStamp {
    let group = BeatGridId::allocate().expect("group id");
    SyncExecutionStamp::new(
        SyncOperationId::first(),
        BeatGridStamp::new(member, BeatGridRevision::first()),
        BeatGridStamp::new(group, BeatGridRevision::first()),
        TopologyStamp::new(group, TopologyRevision::first()),
        LoadGeneration::first(),
        TransportRevision::first(),
    )
}

fn cell() -> PermitCell {
    PermitCell::new(BeatGridId::allocate().expect("member id"))
}

fn bound(arbiter: &Arc<SyncArbiter>) -> SyncGateBinding {
    SyncGateBinding::new(Arc::clone(arbiter), Arc::new(cell()))
}

fn applied(permit: &ArmPermit) -> SyncApplied {
    SyncApplied::builder()
        .stamp(permit.stamp())
        .frontier(
            PresentationFrontier::builder()
                .source(0)
                .output(SessionFrame::new(0))
                .build(),
        )
        .build()
}

/// Claim `cell` for `permit` with its receipt pair reserved in `receipts`.
fn audio_claim<'a, 'r>(
    arbiter: &'a SyncArbiter,
    permit: &ArmPermit,
    cell: &PermitCell,
    receipts: &'r mut SyncReceiptTx,
) -> Result<AudioClaim<'a, 'r>, ClaimError> {
    let reserved = receipts
        .reserve_pair(applied(permit))
        .expect("the test mailbox has room for a pair");
    arbiter.try_claim(permit, cell, reserved)
}

fn permit(binding: &SyncGateBinding) -> ArmPermit {
    binding
        .arbiter()
        .try_control()
        .expect("owner enters")
        .mint_permit(binding.cell(), stamp(binding.cell().member()))
        .expect("permit")
}

#[kithara::test]
fn control_and_audio_claims_have_one_winner() {
    let arbiter = SyncArbiter::new();
    let a = cell();
    let b = cell();
    let (mut a_receipts, _a_inbox) = sync_receipts();
    let (mut b_receipts, _b_inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let a_permit = control
        .mint_permit(&a, stamp(a.member()))
        .expect("a permit");
    let b_permit = control
        .mint_permit(&b, stamp(b.member()))
        .expect("b permit");
    assert!(arbiter.try_control().is_none());
    assert!(matches!(
        audio_claim(&arbiter, &a_permit, &a, &mut a_receipts),
        Err(ClaimError::Busy)
    ));
    drop(control);

    let b_claim = audio_claim(&arbiter, &b_permit, &b, &mut b_receipts).expect("b claims");
    assert!(arbiter.try_control().is_none());
    assert!(matches!(
        audio_claim(&arbiter, &a_permit, &a, &mut a_receipts),
        Err(ClaimError::Busy)
    ));
    b_claim.finish();
    let _control = arbiter.try_control().expect("owner follows b");
}

#[kithara::test]
fn revocation_is_affected_member_only() {
    let arbiter = SyncArbiter::new();
    let a = cell();
    let b = cell();
    let (mut a_receipts, _a_inbox) = sync_receipts();
    let (mut b_receipts, _b_inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let a_permit = control
        .mint_permit(&a, stamp(a.member()))
        .expect("a permit");
    let b_permit = control
        .mint_permit(&b, stamp(b.member()))
        .expect("b permit");
    control.preflight_revoke(&a).expect("preflight a").revoke();
    drop(control);

    assert!(matches!(
        audio_claim(&arbiter, &a_permit, &a, &mut a_receipts),
        Err(ClaimError::StalePermit)
    ));
    audio_claim(&arbiter, &b_permit, &b, &mut b_receipts)
        .expect("unaffected b remains valid")
        .finish();
}

#[kithara::test]
fn retired_member_cannot_rearm_while_another_member_claims() {
    let arbiter = SyncArbiter::new();
    let a = cell();
    let b = cell();
    let (mut a_receipts, _a_inbox) = sync_receipts();
    let (mut b_receipts, _b_inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let a_stamp = stamp(a.member());
    let a_permit = control.mint_permit(&a, a_stamp).expect("a permit");
    let b_permit = control
        .mint_permit(&b, stamp(b.member()))
        .expect("b permit");
    control.retire_cell(&a).expect("a is quiescent");
    assert!(matches!(
        control.mint_permit(&a, a_stamp),
        Err(ControlError::CellRetired)
    ));
    drop(control);
    assert!(matches!(
        audio_claim(&arbiter, &a_permit, &a, &mut a_receipts),
        Err(ClaimError::CellRetired)
    ));
    audio_claim(&arbiter, &b_permit, &b, &mut b_receipts)
        .expect("b still claims")
        .finish();
}

#[kithara::test]
fn claim_releases_owner_only_after_explicit_receipt_completion() {
    let arbiter = SyncArbiter::new();
    let cell = cell();
    let (mut cell_receipts, _cell_inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let permit = control
        .mint_permit(&cell, stamp(cell.member()))
        .expect("permit");
    drop(control);
    let claim = audio_claim(&arbiter, &permit, &cell, &mut cell_receipts).expect("audio claims");
    assert!(arbiter.try_control().is_none());
    claim.finish();
    let _control = arbiter.try_control().expect("owner enters after receipts");
}

#[kithara::test]
fn the_owner_waits_until_the_claim_spends_its_receipt_pair() {
    let arbiter = SyncArbiter::new();
    let cell = cell();
    let (mut receipts, mut inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let permit = control
        .mint_permit(&cell, stamp(cell.member()))
        .expect("permit");
    assert!(matches!(
        audio_claim(&arbiter, &permit, &cell, &mut receipts),
        Err(ClaimError::Busy)
    ));
    drop(control);
    assert_eq!(inbox.next_receipt(), None, "a refused claim writes nothing");

    let claim = audio_claim(&arbiter, &permit, &cell, &mut receipts).expect("audio claims");
    assert_eq!(inbox.next_receipt(), None, "the pair waits for the claim");
    assert!(arbiter.try_control().is_none());
    claim.finish();

    assert_eq!(
        inbox.next_receipt(),
        Some(SyncReceipt::Armed(permit.stamp()))
    );
    assert_eq!(
        inbox.next_receipt(),
        Some(SyncReceipt::Presented(applied(&permit)))
    );
    let _control = arbiter.try_control().expect("owner enters after the pair");
}

#[kithara::test]
fn revision_exhaustion_is_rejected_before_revocation() {
    let arbiter = SyncArbiter::new();
    let permit_spent = cell();
    let control = arbiter.try_control().expect("owner enters");
    permit_spent
        .permit_revision
        .store(u64::MAX, Ordering::Release);
    assert!(matches!(
        control.preflight_revoke(&permit_spent),
        Err(ControlError::RevisionExhausted)
    ));
}

#[kithara::test]
fn abandoned_claim_can_be_tombstoned_after_audio_quiesces() {
    let arbiter = SyncArbiter::new();
    let cell = cell();
    let (mut cell_receipts, _cell_inbox) = sync_receipts();
    let control = arbiter.try_control().expect("owner enters");
    let permit = control
        .mint_permit(&cell, stamp(cell.member()))
        .expect("permit");
    drop(control);
    let claim = audio_claim(&arbiter, &permit, &cell, &mut cell_receipts).expect("audio claims");
    drop(claim);
    assert!(arbiter.try_control().is_none());
    assert!(matches!(
        arbiter.enter_host_control(),
        Err(ControlEnterError::Busy)
    ));
    arbiter.close_quiescent();
    assert!(arbiter.try_control().is_none());
    assert!(matches!(
        arbiter.enter_host_control(),
        Err(ControlEnterError::Closed)
    ));
    assert!(matches!(
        audio_claim(&arbiter, &permit, &cell, &mut cell_receipts),
        Err(ClaimError::Closed)
    ));
}

#[kithara::test]
fn a_reserved_source_parks_its_claim_while_another_member_claims() {
    let arbiter = Arc::new(SyncArbiter::new());
    let a = bound(&arbiter);
    let b = bound(&arbiter);
    let (mut a_receipts, _a_inbox) = sync_receipts();
    let (mut b_receipts, _b_inbox) = sync_receipts();
    let (a_permit, b_permit) = (permit(&a), permit(&b));

    let reservation = a.reserve_source().expect("a player reserves a");
    assert_eq!(a.permit_state(&a_permit), PermitState::Parked);
    assert!(matches!(
        audio_claim(&arbiter, &a_permit, a.cell(), &mut a_receipts),
        Err(ClaimError::SourceParked)
    ));
    assert!(matches!(
        a.reserve_source(),
        Err(ControlError::SourceReserved)
    ));
    audio_claim(&arbiter, &b_permit, b.cell(), &mut b_receipts)
        .expect("an unrelated member claims while a is reserved")
        .finish();
    let control = arbiter.try_control().expect("owner enters");
    assert_eq!(control.source_change(a.cell()), None);
    control
        .retire_cell(b.cell())
        .expect("retirement ignores reservations");
    drop(control);
    drop(reservation);
}

#[kithara::test]
fn a_permit_minted_during_an_edit_waits_for_its_outcome() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    let (mut binding_receipts, _binding_inbox) = sync_receipts();

    let aborted = binding.reserve_source().expect("a player reserves");
    let survives = permit(&binding);
    assert_eq!(binding.permit_state(&survives), PermitState::Parked);
    drop(aborted);
    assert_eq!(
        binding.permit_state(&survives),
        PermitState::Current,
        "an aborted edit leaves the permit its unchanged source"
    );

    let committed = binding.reserve_source().expect("a player reserves again");
    let stale = permit(&binding);
    assert_eq!(binding.permit_state(&stale), PermitState::Parked);
    committed.publish(SourceChange::Discontinuity);
    assert!(matches!(
        audio_claim(&arbiter, &stale, binding.cell(), &mut binding_receipts),
        Err(ClaimError::SourceParked)
    ));
    let control = arbiter.try_control().expect("owner enters");
    let observed = control
        .source_change(binding.cell())
        .expect("the committed edit is pending");
    control
        .preflight_revoke(binding.cell())
        .expect("preflight")
        .revoke();
    control.acknowledge_source_change(binding.cell(), observed);
    drop(control);
    assert_eq!(
        binding.permit_state(&stale),
        PermitState::Withdrawn,
        "a committed edit withdraws the permit minted on the old source"
    );
}

#[kithara::test]
fn an_unpublished_reservation_aborts_without_a_change() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    let (mut binding_receipts, _binding_inbox) = sync_receipts();
    let permit = permit(&binding);
    let before = binding.source_revision();

    drop(binding.reserve_source().expect("reserve"));
    assert_eq!(binding.source_revision(), before);
    assert_eq!(binding.permit_state(&permit), PermitState::Current);
    let control = arbiter.try_control().expect("owner enters");
    assert_eq!(control.source_change(binding.cell()), None);
    assert_eq!(control.current_source(binding.cell()), Ok(before));
    drop(control);
    audio_claim(&arbiter, &permit, binding.cell(), &mut binding_receipts)
        .expect("the unchanged source still claims")
        .finish();
}

#[kithara::test]
fn a_published_change_coalesces_and_holds_the_old_permit_until_withdrawal() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    let (mut binding_receipts, _binding_inbox) = sync_receipts();
    let permit = permit(&binding);
    let before = binding.source_revision();

    binding
        .reserve_source()
        .expect("reserve")
        .publish(SourceChange::Discontinuity);
    binding
        .reserve_source()
        .expect("reserve again")
        .publish(SourceChange::Timing);
    assert_ne!(binding.source_revision(), before);
    assert_eq!(binding.permit_state(&permit), PermitState::Parked);
    assert!(matches!(
        audio_claim(&arbiter, &permit, binding.cell(), &mut binding_receipts),
        Err(ClaimError::SourceParked)
    ));

    let control = arbiter.try_control().expect("owner enters");
    let observed = control
        .source_change(binding.cell())
        .expect("a committed change is pending");
    assert_eq!(
        observed.change(),
        SourceChange::Discontinuity,
        "a later timing change cannot hide the jump"
    );
    assert!(matches!(
        control.mint_permit(binding.cell(), stamp(binding.cell().member())),
        Err(ControlError::SourceChanged)
    ));
    control
        .preflight_revoke(binding.cell())
        .expect("preflight")
        .revoke();
    control.acknowledge_source_change(binding.cell(), observed);
    assert_eq!(binding.permit_state(&permit), PermitState::Withdrawn);
    let fresh = control
        .mint_permit(binding.cell(), stamp(binding.cell().member()))
        .expect("the reconciled source mints again");
    drop(control);
    audio_claim(&arbiter, &fresh, binding.cell(), &mut binding_receipts)
        .expect("a permit for the new source claims")
        .finish();
}

#[kithara::test]
fn a_reservation_does_not_wait_for_an_in_progress_claim() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    let (mut binding_receipts, _binding_inbox) = sync_receipts();
    let permit = permit(&binding);
    let claim = audio_claim(&arbiter, &permit, binding.cell(), &mut binding_receipts)
        .expect("audio claims");
    let reservation = binding
        .reserve_source()
        .expect("a player reserves while the callback claims");
    claim.finish();
    reservation.publish(SourceChange::Discontinuity);
    assert_eq!(binding.permit_state(&permit), PermitState::Parked);
}

#[kithara::test]
fn an_acknowledgement_keeps_a_change_committed_after_its_read() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    binding
        .reserve_source()
        .expect("reserve")
        .publish(SourceChange::Discontinuity);
    let control = arbiter.try_control().expect("owner enters");
    let observed = control
        .source_change(binding.cell())
        .expect("the first change is pending");
    binding
        .reserve_source()
        .expect("a player reserves while the owner reconciles")
        .publish(SourceChange::Discontinuity);
    control.acknowledge_source_change(binding.cell(), observed);
    assert_eq!(
        control
            .source_change(binding.cell())
            .map(PendingSourceChange::change),
        Some(SourceChange::Discontinuity),
        "the change committed after the read is still pending"
    );
    assert!(matches!(
        control.mint_permit(binding.cell(), stamp(binding.cell().member())),
        Err(ControlError::SourceChanged)
    ));
}

#[kithara::test]
fn an_exhausted_source_refuses_its_reservation_and_stays_free() {
    let arbiter = Arc::new(SyncArbiter::new());
    let binding = bound(&arbiter);
    binding.cell().source.store(
        consts::MAX_REVISION << consts::CHANGE_SHIFT,
        Ordering::Release,
    );
    for _ in 0..2 {
        assert!(matches!(
            binding.reserve_source(),
            Err(ControlError::RevisionExhausted)
        ));
    }
    assert!(!binding.cell().reserved.load(Ordering::Acquire));
}
