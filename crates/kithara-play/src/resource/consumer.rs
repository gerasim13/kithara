use kithara_audio::AudioReader;
use kithara_platform::CancelToken;
use kithara_warp::{
    PresentationFrontier, RenderContext, RenderPublisher, RenderReader, supports_playback_rate,
};

use crate::worker::{ServiceClass, TrackPriority};

/// The half of a load a deck slot reads: the decoded-audio reader, the render
/// it publishes, its playback rate and the worker priority the slot sets by
/// the state of its track. The player keeps the rest of the load.
///
/// Fields drop in declaration order, so the reader drops last.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, vis = "pub(crate)")]
pub struct PcmConsumer {
    #[field(with, option_set_some)]
    priority: Option<TrackPriority>,
    #[field(with, option_set_some)]
    render_publisher: Option<RenderPublisher>,
    #[field(with)]
    playback_rate: PlaybackRate,
    reader: ReaderOwner,
}

/// Cancels the wrapped per-track token on drop. A field rather than a
/// `PcmConsumer: Drop` impl so the reader can move out after
/// [`disarm`](CancelGuard::disarm)ing. Passive when `None`.
struct CancelGuard(Option<CancelToken>);

/// Cancels before dropping the reader; tuple fields drop in declaration order.
struct ReaderOwner(CancelGuard, Box<dyn AudioReader>);

/// Media seconds a reader consumes per output second.
pub(crate) enum PlaybackRate {
    /// Its own tempo: no renderer changes its speed.
    Fixed,
    /// The speed its renderer was last asked for.
    Warp(f32),
}

impl PlaybackRate {
    fn apply(&mut self, requested: f32) -> f32 {
        if let Self::Warp(rate) = self {
            *rate = requested;
        }
        f32::from(&*self)
    }

    pub(crate) fn for_warp(speed: f32) -> Self {
        if supports_playback_rate() {
            Self::Warp(speed)
        } else {
            Self::Fixed
        }
    }
}

impl From<&PlaybackRate> for f32 {
    fn from(rate: &PlaybackRate) -> Self {
        match rate {
            PlaybackRate::Fixed => 1.0,
            PlaybackRate::Warp(rate) => *rate,
        }
    }
}

impl CancelGuard {
    /// Disarm so dropping the guard cancels nothing — used when the live reader
    /// outlives this wrapper (handed to the analysis worker), where teardown
    /// rides the analysis run-scope cancel (a parent of this token) instead.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

/// The reader, its per-track cancel disarmed: it outlives the consumer.
impl From<PcmConsumer> for Box<dyn AudioReader> {
    fn from(consumer: PcmConsumer) -> Self {
        let PcmConsumer { reader, .. } = consumer;
        let ReaderOwner(mut cancel, inner) = reader;
        cancel.disarm();
        inner
    }
}

impl PcmConsumer {
    /// A fixed-rate consumer of `reader` that publishes no render and cancels
    /// nothing on drop.
    pub(crate) fn new(reader: Box<dyn AudioReader>) -> Self {
        Self {
            priority: None,
            render_publisher: None,
            playback_rate: PlaybackRate::Fixed,
            reader: ReaderOwner(CancelGuard(None), reader),
        }
    }

    pub(crate) fn apply_playback_rate(&mut self, rate: f32) -> f32 {
        self.playback_rate.apply(rate)
    }

    /// Cancel `cancel` when this consumer drops, before its reader does.
    pub(crate) fn cancel_on_drop(&mut self, cancel: Option<CancelToken>) {
        self.reader.0 = CancelGuard(cancel);
    }

    pub(crate) fn clear_render(&self) {
        if let Some(publisher) = &self.render_publisher {
            publisher.clear();
        }
    }

    pub(crate) fn playback_rate(&self) -> f32 {
        (&self.playback_rate).into()
    }

    pub(crate) fn publish_render(&self, context: &RenderContext, frontier: PresentationFrontier) {
        if let Some(publisher) = &self.render_publisher {
            publisher.publish(context, frontier);
        }
    }

    pub(crate) fn reader(&self) -> &dyn AudioReader {
        &*self.reader.1
    }

    pub(crate) fn reader_mut(&mut self) -> &mut dyn AudioReader {
        &mut *self.reader.1
    }

    pub(crate) fn render_reader(&self) -> Option<RenderReader> {
        self.render_publisher.as_ref().map(RenderPublisher::reader)
    }

    pub(crate) fn set_service_class(&self, class: ServiceClass) {
        if let Some(priority) = &self.priority {
            priority.set(class);
        }
    }
}
