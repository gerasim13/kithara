use std::num::NonZeroU32;

use kithara_audio::{ReadOutcome, SeekBegin};
use kithara_decode::DecodeError;
use kithara_platform::{maybe_send::MaybeSend, sync::Arc, time::Duration};
use kithara_signal::AudioSpec;
use kithara_warp::{PresentationFrontier, RenderContext, RenderReader};

use crate::worker::ServiceClass;

/// A prepared source consumed by the realtime mixer. Source opening stays with playback.
pub trait RenderResource: MaybeSend {
    fn spec(&self) -> AudioSpec;
    fn cached_span(&self) -> Duration;
    fn decoded_frontier(&self) -> Duration;
    /// # Errors
    ///
    /// Returns a decoder error if the prepared source cannot produce samples.
    fn read_planar<'a>(
        &mut self,
        output: &'a mut [&'a mut [f32]],
    ) -> Result<ReadOutcome, DecodeError>;
    fn duration(&self) -> Option<Duration>;
    fn playback_rate(&self) -> f32;
    fn apply_playback_rate(&self, rate: f32) -> f32;
    fn render_reader(&self) -> Option<RenderReader>;
    fn sync_seek(&mut self);
    fn clear_render(&self);
    fn seek_handle(&self) -> Option<Arc<dyn SeekBegin>>;
    fn set_host_sample_rate(&self, sample_rate: NonZeroU32);
    fn set_service_class(&self, class: ServiceClass);
    fn publish_render(&self, context: &RenderContext, frontier: PresentationFrontier);
}
