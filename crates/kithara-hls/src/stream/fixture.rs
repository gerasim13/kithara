//! Test-only coord scaffolding, shared by the coord and peer unit tests.

/// Gated in one place so nothing below reads as production surface —
/// per-file readers, arch lints included, see a single test-only block.
#[cfg(test)]
mod support {
    use std::sync::OnceLock;

    use kithara_abr::{Abr, AbrController, AbrHandle, AbrSettings};
    use kithara_assets::{AssetResource, AssetSource, AssetStoreBuilder, StorageBackend};
    use kithara_events::{AbrMode, DeferredBus, EventBus, VariantIndex};
    use kithara_platform::{
        CancelToken,
        sync::{Arc, ThreadGate},
        time::Duration,
    };
    use kithara_stream::{AudioCodec, ContainerFormat, PlayheadState, TimelineState};

    use crate::{
        config::SizeProbeMethod,
        ids::SegmentIndex,
        playlist::{PlaylistState, SegmentState, VariantState},
        segment::{
            InitSegment, MediaSegment, Segment, SegmentContent, SegmentSize, SegmentSlotState,
            SegmentSource,
        },
        signal::SizeSignal,
        stream::{HlsCoord, HlsCoordEnv},
        variant::{PlanCtx, VariantParts},
    };

    /// One variant of a fixture track.
    #[derive(Clone, Copy)]
    pub(crate) struct VariantSpec {
        pub(crate) codec: AudioCodec,
        pub(crate) container: ContainerFormat,
        pub(crate) segments: usize,
        pub(crate) segment_size: u64,
        segment_duration: Duration,
        has_init: bool,
        /// Whether the playlist declares the byte length (`#EXT-X-BYTERANGE`).
        /// A variant without it carries routeable placeholders, so anything that
        /// needs a real length has to probe for it.
        pub(crate) exact_sizes: bool,
    }

    impl VariantSpec {
        pub(crate) fn new(codec: AudioCodec, container: ContainerFormat, segments: usize) -> Self {
            Self {
                codec,
                container,
                segments,
                segment_size: 100,
                segment_duration: Duration::from_secs(2),
                has_init: false,
                exact_sizes: true,
            }
        }

        pub(crate) fn with_init(mut self) -> Self {
            self.has_init = true;
            self
        }

        pub(crate) fn with_segment_duration(mut self, duration: Duration) -> Self {
            self.segment_duration = duration;
            self
        }

        pub(crate) fn placeholder_sizes(mut self) -> Self {
            self.exact_sizes = false;
            self
        }
    }

    /// A coord over synthesised variants, wired to a real ABR controller — the
    /// shape unit tests need to drive commits, dispatch and the peer without a
    /// server.
    pub(crate) struct CoordFixture {
        pub(crate) abr: AbrHandle,
        pub(crate) bus: EventBus,
        pub(crate) coord: Arc<HlsCoord>,
        pub(crate) ctx: PlanCtx,
        /// The registered ABR participant. The controller only holds it weakly,
        /// so a fixture that dropped it would silently stop ticking.
        pub(crate) _peer: Arc<dyn Abr>,
    }

    pub(crate) fn segment_url(variant: usize, seg: usize) -> url::Url {
        format!("https://example.com/v{variant}-seg{seg}.m4s")
            .parse()
            .expect("fixture segment url")
    }

    fn init_url(variant: usize) -> url::Url {
        format!("https://example.com/v{variant}-init.mp4")
            .parse()
            .expect("fixture init url")
    }

    fn plan_ctx(
        bus: &EventBus,
        cancel: &CancelToken,
        signal: &SizeSignal,
        budget: usize,
    ) -> PlanCtx {
        let store = Arc::new(
            AssetStoreBuilder::default()
                .backend(StorageBackend::Memory)
                .cancel(cancel.clone())
                .build(),
        );
        PlanCtx {
            bus: bus.clone(),
            fetch: crate::variant::test_transport(),
            prefetch_budget: budget,
            master_cancel: cancel.clone(),
            scope: store
                .scope::<crate::Hls>(&AssetSource::Remote {
                    url: "https://example.com/master.m3u8"
                        .parse()
                        .expect("master url"),
                    discriminator: Some("coord-test".to_owned()),
                })
                .expect("coord asset scope"),
            seek_epoch: 0,
            look_ahead_bytes: None,
            look_ahead_segments: None,
            headers: None,
            size_probe_method: SizeProbeMethod::Head,
            signal: signal.clone(),
        }
    }

    fn playlist_of(specs: &[VariantSpec]) -> Arc<PlaylistState> {
        Arc::new(PlaylistState::new(
            specs
                .iter()
                .enumerate()
                .map(|(v_idx, spec)| VariantState {
                    codec: Some(spec.codec),
                    container: Some(spec.container),
                    init_url: spec.has_init.then(|| init_url(v_idx)),
                    segments: (0..spec.segments)
                        .map(|seg| SegmentState {
                            url: segment_url(v_idx, seg),
                            duration: spec.segment_duration,
                            byte_range_len: spec.exact_sizes.then_some(spec.segment_size),
                            index: SegmentIndex::try_new(seg, spec.segments).expect("in-bounds"),
                        })
                        .collect(),
                })
                .collect(),
        ))
    }

    fn variants_of(
        specs: &[VariantSpec],
        playlist: &PlaylistState,
        ctx: &PlanCtx,
    ) -> Arc<[Arc<crate::variant::HlsVariant>]> {
        Arc::from(
            specs
                .iter()
                .enumerate()
                .map(|(v_idx, spec)| {
                    let init = spec.has_init.then(|| {
                        let url = init_url(v_idx);
                        Segment::Init(InitSegment {
                            resource_id: ctx
                                .scope
                                .key(&AssetResource::Url(url.clone()))
                                .expect("init key"),
                            url,
                            state: SegmentSlotState::missing(),
                            size: SegmentSize::seed(32),
                            content: SegmentContent::Plain,
                            source: SegmentSource::new(
                                ctx.scope.store().clone(),
                                ctx.master_cancel.child(),
                            ),
                        })
                    });
                    let segments = (0..spec.segments)
                        .map(|seg| {
                            let url = segment_url(v_idx, seg);
                            Segment::Media(MediaSegment {
                                resource_id: ctx
                                    .scope
                                    .key(&AssetResource::Url(url.clone()))
                                    .expect("segment key"),
                                url,
                                state: SegmentSlotState::missing(),
                                size: if spec.exact_sizes {
                                    SegmentSize::seed(spec.segment_size)
                                } else {
                                    SegmentSize::placeholder(spec.segment_size)
                                },
                                content: SegmentContent::Plain,
                                source: SegmentSource::new(
                                    ctx.scope.store().clone(),
                                    ctx.master_cancel.child(),
                                ),
                                decode_time: spec.segment_duration.saturating_mul(
                                    u32::try_from(seg).expect("fixture segment index fits u32"),
                                ),
                                duration: spec.segment_duration,
                            })
                        })
                        .collect();
                    VariantParts {
                        init,
                        segments,
                        codec: playlist.variant_codec(v_idx),
                        container: playlist.variant_container(v_idx),
                    }
                    .into_variant(v_idx, ctx)
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Build a coord over `specs`, with `abr` already reporting variant 0 current.
    /// `peer` is the ABR participant registered with the controller — pass the
    /// production [`HlsPeer`](crate::peer::HlsPeer) to drive `poll_next`, or a
    /// stub when only the coord is under test.
    pub(crate) fn coord_fixture(
        specs: &[VariantSpec],
        prefetch_budget: usize,
        peer: &Arc<dyn Abr>,
    ) -> CoordFixture {
        let bus = EventBus::new(8);
        let cancel = CancelToken::never();
        let signal = SizeSignal::new(Arc::new(ThreadGate::default()), Arc::new(OnceLock::new()));
        let ctx = plan_ctx(&bus, &cancel, &signal, prefetch_budget);
        let playlist = playlist_of(specs);
        let variants = variants_of(specs, playlist.as_ref(), &ctx);
        let controller = Arc::new(AbrController::new(
            AbrSettings::default(),
            CancelToken::never(),
        ));
        let abr = controller.register(peer);
        let coord = Arc::new(HlsCoord::new(
            HlsCoordEnv {
                scope: ctx.scope.clone(),
                fetch: crate::variant::test_transport(),
                cancel,
                headers: None,
                emit: Arc::new(DeferredBus::new(bus.clone(), 8)),
                signal,
            },
            Arc::new(PlayheadState::new()),
            Arc::new(TimelineState::new()),
            abr.clone(),
            variants,
            playlist,
        ));
        CoordFixture {
            abr,
            bus,
            coord,
            ctx,
            _peer: Arc::clone(peer),
        }
    }

    /// Variant metadata for an `Abr` stub, matching `specs` one for one.
    pub(crate) fn variant_infos(specs: &[VariantSpec]) -> Vec<kithara_events::VariantInfo> {
        specs
            .iter()
            .enumerate()
            .map(|(v_idx, spec)| kithara_events::VariantInfo {
                variant_index: VariantIndex::new(v_idx),
                bandwidth_bps: Some(128_000),
                codecs: None,
                container: Some(format!("{:?}", spec.container)),
                duration: kithara_events::VariantDuration::Segmented(
                    (0..spec.segments).map(|_| spec.segment_duration).collect(),
                ),
                name: None,
            })
            .collect()
    }

    /// The ABR participant a coord-only test registers: it owns the state the
    /// controller mutates and nothing else.
    pub(crate) struct StubAbrPeer {
        pub(crate) state: Arc<kithara_abr::AbrState>,
        pub(crate) variants: Vec<kithara_events::VariantInfo>,
    }

    impl StubAbrPeer {
        pub(crate) fn new(specs: &[VariantSpec]) -> Arc<dyn Abr> {
            Arc::new(Self {
                state: Arc::new(kithara_abr::AbrState::new(AbrMode::Auto(Some(
                    VariantIndex::new(0),
                )))),
                variants: variant_infos(specs),
            })
        }
    }

    impl Abr for StubAbrPeer {
        fn state(&self) -> Option<Arc<kithara_abr::AbrState>> {
            Some(Arc::clone(&self.state))
        }

        fn variants(&self) -> Vec<kithara_events::VariantInfo> {
            self.variants.clone()
        }
    }
}

pub(crate) use support::*;
