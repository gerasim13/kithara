use kithara_decode::TrackMetadata;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_stream::{Activity, ActivityWriter, PlayheadWrite};
pub(in crate::audio) struct AudioContext {
    pub(in crate::audio) playhead: Arc<dyn PlayheadWrite>,
    pub(in crate::audio) bus: EventBus,
    pub(in crate::audio) metadata: TrackMetadata,
    pub(in crate::audio) abr: Option<kithara_abr::AbrHandle>,
    pub(in crate::audio) activity: Activity,
    pub(in crate::audio) activity_writer: Option<ActivityWriter>,
    pub(in crate::audio) cancel: CancelToken,
}
