use kithara_assets::{ReadSide, WriteSide};
use kithara_net::{NetError, Retryability};
use kithara_storage::ResourceStatus;
use kithara_test_utils::kithara;

use super::{FilePhase, ResourceEngine};
use crate::config::FetchOutcome;

impl ResourceEngine {
    /// Apply the result of a streaming fetch to the resource.
    ///
    /// * commits the resource once the full byte space is covered
    /// * on a transient error with bytes already received, leaves the
    ///   resource active so the peer's next `poll_next` issues a
    ///   Range GET for the remaining gap
    /// * on a terminal error (non-retryable, or hard failure with no
    ///   bytes received), fails+evicts the resource and reports it to
    ///   the lifecycle sink
    ///
    /// Header capture (`Content-Length` / `Content-Type`) happens
    /// eagerly in `on_response`, not here, so a reader blocked on the
    /// first byte sees the seeded total the instant `write_at` fires.
    #[kithara::probe(resume_from, bytes_written)]
    pub(crate) fn finalize_fetch(
        &self,
        resume_from: u64,
        bytes_written: u64,
        total_bytes: Option<u64>,
        err: Option<&NetError>,
    ) {
        if let Some(e) = err {
            self.finalize_error(resume_from, bytes_written, e);
            return;
        }

        if self.next_gap_start(total_bytes).is_some() {
            return;
        }

        self.finalize_commit(total_bytes.unwrap_or(resume_from + bytes_written));
    }

    fn committed(&self) -> bool {
        matches!(self.reader.status(), ResourceStatus::Committed { .. })
    }

    /// Commit the fully covered resource, or adopt the commit a racing
    /// writer already made.
    fn finalize_commit(&self, final_len: u64) {
        let Some(writer) = self.take_writer() else {
            // Writer already consumed (committed by a sibling/race) — the
            // resource is final; just advance the FSM.
            let final_len = self.reader.len();
            self.set_phase(FilePhase::Complete, final_len);
            self.notify_fetch_complete(FetchOutcome::Committed { final_len });
            return;
        };
        match writer.commit(Some(final_len)) {
            Ok(reader) => {
                let final_len = reader.len();
                self.set_phase(FilePhase::Complete, final_len);
                self.notify_fetch_complete(FetchOutcome::Committed { final_len });
            }
            Err(e) => {
                let reason = e.to_string();
                self.fail_and_evict(&reason);
                self.sink.error(&reason);
                self.notify_fetch_complete(FetchOutcome::CommitFailed { reason });
            }
        }
    }

    /// A cancelled fetch owns nothing but its own writer.
    ///
    /// Failing the writer is generation-scoped and race-safe; **evicting the
    /// resource is not**, and another writer — a new seek epoch fetching the
    /// same segment — may already be filling it. So a cancel never evicts,
    /// and it is not an error either: the outcome says it was cancelled.
    fn finalize_cancel(&self, e: &NetError) {
        let committed = self.committed();
        if !committed && let Some(writer) = self.take_writer() {
            writer.fail(e.to_string());
        }
        self.notify_fetch_complete(FetchOutcome::Cancelled { committed });
    }

    /// A transient error with bytes already received leaves the resource
    /// active, so the peer's next `poll_next` issues a Range GET for the
    /// remaining gap and nothing is reported — including under a cancel,
    /// where those bytes are still a resume point for the next open.
    fn finalize_error(&self, resume_from: u64, bytes_written: u64, e: &NetError) {
        let terminal =
            e.retryability() == Retryability::Fatal || (resume_from == 0 && bytes_written == 0);
        if !terminal {
            return;
        }
        if self.identity.cancel.is_cancelled() || matches!(e, NetError::Cancelled) {
            self.finalize_cancel(e);
            return;
        }
        let reason = e.to_string();
        self.fail_and_evict(&reason);
        self.sink.error(&reason);
        self.notify_fetch_complete(FetchOutcome::Failed { error: e.clone() });
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::{
        AcquisitionResult, AssetResource, AssetResourceState, AssetSource, AssetStore,
        AssetStoreBuilder, ChunkSink, ProcessCtx, RawWriteHandle, ResourceKey, ResourceProcessor,
        StorageBackend,
    };
    use kithara_events::EventBus;
    use kithara_platform::{
        CancelScope, CancelToken,
        sync::{Arc, Mutex},
    };
    use kithara_stream::{PlayheadState, TimelineState};
    use num_traits::AsPrimitive;
    use url::Url;

    use super::*;
    use crate::{
        File,
        config::FetchCompleteFn,
        coord::FileCoord,
        engine::{EngineIdentity, LifecycleSink},
        session::FileSourceCtx,
    };

    const BYTES: &[u8] = b"one-segment-worth-of-bytes";

    /// Rejects every chunk, so the processing pass fails the commit.
    #[derive(Debug)]
    struct RejectingProcessor;

    impl ResourceProcessor for RejectingProcessor {
        fn begin(&self) -> Box<dyn ChunkSink> {
            Box::new(Self)
        }

        fn identity(&self) -> &[u8] {
            b"rejecting-processor"
        }
    }

    impl ChunkSink for RejectingProcessor {
        fn process(
            &mut self,
            _input: &[u8],
            _output: &mut [u8],
            _last: bool,
        ) -> Result<usize, String> {
            Err("processor rejected the bytes".to_string())
        }
    }

    struct Fixture {
        engine: Arc<ResourceEngine>,
        key: ResourceKey,
        outcomes: Arc<Mutex<Vec<FetchOutcome>>>,
        raw: RawWriteHandle,
        scope: CancelScope,
        store: AssetStore,
    }

    impl Fixture {
        fn new(process: Option<ProcessCtx>) -> Self {
            let scope = CancelScope::new(None);
            let store = AssetStoreBuilder::default()
                .backend(StorageBackend::Memory)
                .cancel(CancelToken::never())
                .build();
            let key = test_key(&store);
            let identity = EngineIdentity {
                store: store.clone(),
                key: key.clone(),
                process,
                cancel: scope.token(),
            };
            let AcquisitionResult::Pending(writer) = identity.acquire().expect("acquire") else {
                panic!("a fresh acquire must be Pending");
            };
            let reader = writer.reader();
            let raw = writer.raw_write_handle();
            let source = Arc::new(FileSourceCtx::new(
                Arc::new(FileCoord::new(
                    Arc::new(PlayheadState::new()),
                    Arc::new(TimelineState::new()),
                )),
                EventBus::new(16),
                reader.clone(),
                false,
                None,
            ));
            let outcomes: Arc<Mutex<Vec<FetchOutcome>>> = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&outcomes);
            let engine = Arc::new(
                ResourceEngine::builder()
                    .identity(identity)
                    .reader(reader)
                    .writer(writer)
                    .raw(raw.clone())
                    .sink(Arc::clone(&source) as Arc<dyn LifecycleSink>)
                    .initial_phase(FilePhase::Init)
                    .on_complete(
                        Arc::new(move |outcome| recorder.lock().push(outcome)) as FetchCompleteFn
                    )
                    .build(),
            );
            Self {
                engine,
                key,
                outcomes,
                raw,
                scope,
                store,
            }
        }

        /// Hand the resource to a racing writer that commits it, leaving the
        /// engine without a writer — the committed-by-race shape.
        fn commit_elsewhere(&self) {
            let writer = self.engine.take_writer().expect("engine writer");
            writer
                .raw_write_handle()
                .write_at(0, BYTES)
                .expect("racing write");
            writer.commit(Some(len())).expect("racing commit");
        }

        fn outcome(&self) -> Option<FetchOutcome> {
            self.outcomes.lock().first().cloned()
        }

        fn reported(&self) -> usize {
            self.outcomes.lock().len()
        }

        fn state(&self) -> AssetResourceState {
            self.store
                .resource_state(&self.key)
                .expect("resource state")
        }

        fn write_body(&self) {
            self.raw.write_at(0, BYTES).expect("body write");
        }
    }

    fn test_key(store: &AssetStore) -> ResourceKey {
        store
            .scope::<File>(&AssetSource::Remote {
                url: Url::parse("https://example.com/settle.dat").expect("test URL"),
                discriminator: Some("settle-test".to_string()),
            })
            .expect("test scope")
            .key(&AssetResource::Source {
                extension: "dat".to_string(),
            })
            .expect("test key")
    }

    fn len() -> u64 {
        BYTES.len().as_()
    }

    #[kithara::test]
    fn a_full_body_reports_the_committed_length() {
        let fixture = Fixture::new(None);
        fixture.write_body();

        fixture.engine.finalize_fetch(0, len(), Some(len()), None);

        assert!(
            matches!(fixture.outcome(), Some(FetchOutcome::Committed { final_len }) if final_len == Some(len())),
            "the committed length is read back off the post-commit reader, got {:?}",
            fixture.outcome()
        );
    }

    #[kithara::test]
    fn a_racing_commit_is_adopted_not_re_committed() {
        let fixture = Fixture::new(None);
        fixture.commit_elsewhere();

        fixture.engine.finalize_fetch(0, len(), Some(len()), None);

        assert!(
            matches!(fixture.outcome(), Some(FetchOutcome::Committed { final_len }) if final_len == Some(len())),
            "a resource committed under us is still a commit, got {:?}",
            fixture.outcome()
        );
    }

    #[kithara::test]
    fn a_terminal_transport_error_reports_the_error_and_evicts() {
        let fixture = Fixture::new(None);

        fixture
            .engine
            .finalize_fetch(0, 0, None, Some(&NetError::Decode("bad body".to_string())));

        assert!(
            matches!(
                fixture.outcome(),
                Some(FetchOutcome::Failed {
                    error: NetError::Decode(_)
                })
            ),
            "the typed transport error reaches the caller, which owns the retry policy, got {:?}",
            fixture.outcome()
        );
        assert_eq!(fixture.state(), AssetResourceState::Missing);
    }

    #[kithara::test]
    fn a_transient_error_mid_body_reports_nothing() {
        let fixture = Fixture::new(None);
        fixture.raw.write_at(0, &BYTES[..4]).expect("partial write");

        fixture
            .engine
            .finalize_fetch(0, 4, Some(len()), Some(&NetError::Timeout));

        assert_eq!(
            fixture.reported(),
            0,
            "the fetch is not over: the peer resumes the gap, so there is no outcome to report"
        );
        assert!(matches!(fixture.state(), AssetResourceState::Active));
    }

    #[kithara::test]
    fn a_rejected_commit_reports_commit_failed() {
        let fixture = Fixture::new(Some(Arc::new(RejectingProcessor) as ProcessCtx));
        fixture.write_body();

        fixture.engine.finalize_fetch(0, len(), Some(len()), None);

        assert!(
            matches!(fixture.outcome(), Some(FetchOutcome::CommitFailed { .. })),
            "a processing pass that rejects the bytes is not a transport failure, got {:?}",
            fixture.outcome()
        );
        assert_eq!(fixture.state(), AssetResourceState::Missing);
    }

    #[kithara::test]
    fn a_cancel_before_commit_reports_uncommitted() {
        let fixture = Fixture::new(None);
        fixture.write_body();
        fixture.scope.cancel();

        fixture
            .engine
            .finalize_fetch(0, len(), Some(len()), Some(&NetError::Cancelled));

        assert!(
            matches!(
                fixture.outcome(),
                Some(FetchOutcome::Cancelled { committed: false })
            ),
            "nothing committed, so the partial resource is ours to fail, got {:?}",
            fixture.outcome()
        );
        assert!(
            matches!(fixture.state(), AssetResourceState::Failed { .. }),
            "our writer is failed — but the resource is NOT evicted, because a \
             new epoch fetching the same key may already own it"
        );
    }

    #[kithara::test]
    fn a_cancel_mid_body_keeps_the_resume_point() {
        let fixture = Fixture::new(None);
        fixture.raw.write_at(0, &BYTES[..4]).expect("partial write");
        fixture.scope.cancel();

        fixture
            .engine
            .finalize_fetch(0, 4, Some(len()), Some(&NetError::Timeout));

        assert_eq!(
            fixture.reported(),
            0,
            "a resumable error is not an end, cancelled or not"
        );
        assert!(
            matches!(fixture.state(), AssetResourceState::Active),
            "a cancel must not throw away the bytes the next open would resume from"
        );
    }

    #[kithara::test]
    fn a_cancel_after_a_racing_commit_keeps_the_resource() {
        let fixture = Fixture::new(None);
        fixture.commit_elsewhere();
        fixture.scope.cancel();

        fixture
            .engine
            .finalize_fetch(0, 0, Some(len()), Some(&NetError::Cancelled));

        assert!(
            matches!(
                fixture.outcome(),
                Some(FetchOutcome::Cancelled { committed: true })
            ),
            "the caller must be able to tell this cancel apart from a lost fetch, got {:?}",
            fixture.outcome()
        );
        assert!(
            matches!(
                fixture.state(),
                AssetResourceState::Committed { final_len: Some(_) }
            ),
            "our cancel must not evict bytes another writer committed"
        );
    }

    #[kithara::test]
    fn the_outcome_is_reported_once() {
        let fixture = Fixture::new(None);
        fixture.write_body();

        fixture.engine.finalize_fetch(0, len(), Some(len()), None);
        fixture.engine.finalize_fetch(0, len(), Some(len()), None);
        fixture
            .engine
            .finalize_fetch(0, 0, Some(len()), Some(&NetError::Cancelled));

        assert_eq!(
            fixture.reported(),
            1,
            "one fetch has one end, however many times the driver settles it"
        );
    }
}
