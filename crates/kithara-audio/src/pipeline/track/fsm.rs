#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    use kithara_stream::MediaInfo;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::pipeline::{
        consumer::{ConsumerPhase, FailureSource},
        rebuild::{RebuildState, RecreateCause, RecreateNext, RecreateState, state::BuildId},
        seek::{ApplySeekState, ResumeState, SeekContext, SeekMode, SeekRequest},
        track::{Track, TrackFailure, WaitContext, WaitState},
    };

    fn seek_at(secs: u64) -> SeekRequest {
        SeekRequest {
            seek: SeekContext {
                epoch: 1,
                target: Duration::from_secs(secs),
            },
            ..Default::default()
        }
    }

    fn recreate_state() -> RecreateState {
        RecreateState {
            cause: RecreateCause::FormatBoundary,
            media_info: MediaInfo::default(),
            next: RecreateNext::Decode,
            offset: 0,
        }
    }

    fn rebuild_state() -> RebuildState {
        RebuildState {
            build: BuildId::fixture(1),
            recreate: recreate_state(),
            started_seek_epoch: 0,
            superseded_seek: None,
        }
    }

    #[kithara::test]
    fn is_terminal_for_each_phase() {
        let non_terminal = [
            Track::<Decoding>::new(()).erase(),
            Track::<SeekRequested>::new(seek_at(5)).erase(),
            Track::<WaitingForSource>::new(WaitState {
                context: WaitContext::Playback,
                reason: WaitingReason::Waiting,
            })
            .erase(),
            Track::<ApplyingSeek>::new(ApplySeekState {
                mode: SeekMode::Direct { target_byte: None },
                request: seek_at(5),
            })
            .erase(),
            Track::<RecreatingDecoder>::new(recreate_state()).erase(),
            Track::<RebuildingDecoder>::new(rebuild_state()).erase(),
            Track::<AwaitingResume>::new(ResumeState {
                seek: SeekContext {
                    epoch: 1,
                    target: Duration::from_secs(5),
                },
                anchor_offset: None,
                anchor_variant_index: None,
                trim_head: false,
            })
            .erase(),
            Track::<AtEof>::new(()).erase(),
        ];
        for (idx, fsm) in non_terminal.iter().enumerate() {
            assert!(!fsm.is_terminal(), "expected non-terminal phase #{idx}");
        }

        assert!(
            Track::<Failed>::new(TrackFailure::SourceCancelled)
                .erase()
                .is_terminal()
        );
    }

    #[kithara::test]
    fn map_source_phase_table() {
        assert_eq!(
            map_source_phase(SourcePhase::Waiting),
            Some(WaitingReason::Waiting)
        );
        assert_eq!(
            map_source_phase(SourcePhase::WaitingDemand),
            Some(WaitingReason::WaitingDemand)
        );
        assert_eq!(
            map_source_phase(SourcePhase::WaitingMetadata),
            Some(WaitingReason::WaitingMetadata)
        );
        assert_eq!(map_source_phase(SourcePhase::Ready), None);
        assert_eq!(map_source_phase(SourcePhase::Eof), None);
        assert_eq!(map_source_phase(SourcePhase::Seeking), None);
        assert_eq!(map_source_phase(SourcePhase::Cancelled), None);
    }

    #[kithara::test]
    fn consumer_phase_terminal() {
        assert!(!ConsumerPhase::Buffering.is_terminal());
        assert!(!ConsumerPhase::Playing.is_terminal());
        assert!(!ConsumerPhase::SeekPending { epoch: 1 }.is_terminal());
        assert!(ConsumerPhase::AtEof.is_terminal());
        assert!(
            ConsumerPhase::Failed {
                source: FailureSource::Producer
            }
            .is_terminal()
        );
    }

    #[kithara::test]
    fn seek_context_copy_and_eq() {
        let ctx = SeekContext {
            epoch: 42,
            target: Duration::from_millis(500),
        };
        let copy = ctx;
        assert_eq!(ctx, copy);
        assert_eq!(copy.epoch, 42);
        assert_eq!(copy.target, Duration::from_millis(500));
    }

    #[kithara::test]
    fn at_eof_allows_seek_transition() {
        let fsm = Track::<AtEof>::new(()).erase();
        assert!(!fsm.is_terminal());
        assert!(matches!(fsm, CurrentFsm::AtEof(_)));
    }
}
