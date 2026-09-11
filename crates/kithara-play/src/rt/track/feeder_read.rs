use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

use kithara_audio::RevisionFloorStatus;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_signal::FrameCount;
use kithara_warp::{PresentationFrontier, RenderContext, RenderReader};

use super::feeder::{PlayerResource, ReadOutcome, activation_prefix, combine_reads};
use crate::{bridge::RtMetrics, resource::RenderActivation, worker::ServiceClass};

impl PlayerResource {
    /// Read audio frames into the output buffers for the given range.
    ///
    /// Fills internal scratch buffers from the underlying resource as needed,
    /// then copies the requested frames into `output`. Shifts any remaining
    /// data to the front of the scratch buffers.
    ///
    /// When the underlying reader temporarily returns zero frames without EOF
    /// (for example, while an async seek is still settling), this method
    /// zero-fills the requested range and reports [`ReadOutcome::Full`].
    /// That silence is not a terminal condition and must not trigger track
    /// advancement.
    pub fn read(
        &mut self,
        output: &mut [&mut [f32]],
        range: Range<usize>,
        metrics: &RtMetrics,
    ) -> ReadOutcome {
        self.read_with_context(None, None, output, range, metrics).0
    }

    pub(crate) fn read_with_context(
        &mut self,
        context: Option<&RenderContext>,
        track_id: Option<TrackId>,
        output: &mut [&mut [f32]],
        range: Range<usize>,
        metrics: &RtMetrics,
    ) -> (ReadOutcome, u64) {
        let Some(context) = context else {
            return self.read_current(None, track_id, output, range, metrics);
        };
        let Some(activation) = self.resource.get().render_activation() else {
            return self.read_current(Some(context), track_id, output, range, metrics);
        };
        if activation.revision <= self.render_revision_floor {
            return self.read_current(Some(context), track_id, output, range, metrics);
        }
        let Some(prefix_frames) = activation_prefix(context, activation) else {
            return self.read_current(Some(context), track_id, output, range, metrics);
        };
        let frames_to_read = range.end - range.start;
        if prefix_frames == 0 {
            self.present_scheduled_seek();
            if let Some(required) = NonZeroUsize::new(frames_to_read) {
                let _status = self.sync_render_revision(activation, required);
            }
            return self.read_current(Some(context), track_id, output, range, metrics);
        }

        let prefix_end = range.start.saturating_add(prefix_frames);
        let Some(prefix_context) = context.for_output_range(0..prefix_frames) else {
            return (ReadOutcome::Failed, 0);
        };
        let (prefix, prefix_source_frames) = self.read_current(
            Some(&prefix_context),
            track_id,
            output,
            range.start..prefix_end,
            metrics,
        );
        let ReadOutcome::Full {
            frames: copied_prefix,
        } = prefix
        else {
            return (prefix, prefix_source_frames);
        };
        if copied_prefix != prefix_frames {
            return (prefix, prefix_source_frames);
        }

        let suffix_frames = frames_to_read - prefix_frames;
        self.present_scheduled_seek();
        let required = NonZeroUsize::new(suffix_frames).expect("activation suffix is non-zero");
        let _status = self.sync_render_revision(activation, required);
        let Some(suffix_context) = context.for_output_range(prefix_frames..frames_to_read) else {
            return (ReadOutcome::Failed, prefix_source_frames);
        };
        let (suffix, suffix_source_frames) = self.read_current(
            Some(&suffix_context),
            track_id,
            output,
            prefix_end..range.end,
            metrics,
        );
        combine_reads(
            prefix_frames,
            prefix_source_frames,
            suffix,
            suffix_source_frames,
        )
    }

    fn read_current(
        &mut self,
        context: Option<&RenderContext>,
        track_id: Option<TrackId>,
        output: &mut [&mut [f32]],
        range: Range<usize>,
        metrics: &RtMetrics,
    ) -> (ReadOutcome, u64) {
        let frames_to_read = range.len();
        let eof_reached = self.fill_scratch(frames_to_read, metrics);

        if self.write_len == 0 && self.failed && !self.eof_seen {
            for ch in output.iter_mut() {
                ch[range.clone()].fill(0.0);
            }
            return (ReadOutcome::Failed, 0);
        }

        if self.write_len > 0 {
            let frames_to_write = frames_to_read.min(self.write_len);
            let tail_size = self.write_len - frames_to_write;

            if output.len() >= Self::STEREO_CHANNELS {
                output[0][range.start..range.start + frames_to_write]
                    .copy_from_slice(&self.channel_buffers[0][..frames_to_write]);
                output[1][range.start..range.start + frames_to_write]
                    .copy_from_slice(&self.channel_buffers[1][..frames_to_write]);
            }

            let Some(source_frames) = self.consume_source(frames_to_write, context, track_id)
            else {
                metrics.record_decode_error();
                self.failed = true;
                self.last_source_end = None;
                self.source_spans.clear();
                self.write_len = 0;
                self.write_pos = 0;
                return (ReadOutcome::Failed, 0);
            };

            if tail_size > 0 {
                self.channel_buffers[0]
                    .copy_within(frames_to_write..frames_to_write + tail_size, 0);
                self.channel_buffers[1]
                    .copy_within(frames_to_write..frames_to_write + tail_size, 0);
            }

            self.write_len -= frames_to_write;
            self.write_pos = tail_size;

            let outcome = if frames_to_write == frames_to_read {
                ReadOutcome::Full {
                    frames: frames_to_write,
                }
            } else if eof_reached {
                ReadOutcome::Partial {
                    frames: frames_to_write,
                }
            } else {
                metrics.record_underrun();
                for ch in output.iter_mut() {
                    ch[range.start + frames_to_write..range.end].fill(0.0);
                }
                ReadOutcome::Full {
                    frames: frames_to_write,
                }
            };
            (outcome, source_frames)
        } else if eof_reached {
            (ReadOutcome::Eof, 0)
        } else {
            metrics.record_underrun();
            let range_len = range.len();
            for ch in output.iter_mut() {
                ch[range.start..range.start + range_len].fill(0.0);
            }
            (ReadOutcome::Full { frames: 0 }, 0)
        }
    }

    fn sync_render_revision(
        &mut self,
        activation: RenderActivation,
        required_frames: NonZeroUsize,
    ) -> RevisionFloorStatus {
        let revision = activation.revision;
        if revision <= self.render_revision_floor {
            return RevisionFloorStatus::Current;
        }
        let status = self.resource.get_mut().sync_render_revision(
            revision,
            required_frames,
            self.last_source_end,
        );
        if status == RevisionFloorStatus::WaitingForReplacement {
            return status;
        }
        self.render_revision_floor = revision;
        if self.source_spans.iter().any(|span| {
            span.source
                .is_some_and(|source| source.render_revision() < revision)
        }) {
            self.source_spans.clear();
            self.write_len = 0;
            self.write_pos = 0;
        }
        status
    }

    fn present_scheduled_seek(&mut self) {
        let Some(epoch) = self.scheduled_seek_epoch else {
            return;
        };
        match self.resource.get_mut().present_seek(epoch) {
            kithara_audio::SeekPresentation::Presented
            | kithara_audio::SeekPresentation::Current => {
                self.scheduled_seek_epoch = None;
                self.source_spans.clear();
                self.write_len = 0;
                self.write_pos = 0;
                self.last_source_end = None;
                self.eof_seen = false;
                self.failed = false;
            }
            kithara_audio::SeekPresentation::Superseded => {
                self.scheduled_seek_epoch = None;
            }
        }
    }

    pub(crate) const fn schedule_seek(&mut self, epoch: u64) {
        self.scheduled_seek_epoch = Some(epoch);
    }

    pub(crate) fn render_reader(&self) -> Option<RenderReader> {
        self.resource.get().render_reader()
    }

    /// Drop everything buffered ahead of a seek the control thread began. Lock-free: the reader
    /// picks up the epoch itself via `sync_seek`.
    pub fn reset_for_seek(&mut self) {
        self.resource.get_mut().sync_seek();
        self.write_len = 0;
        self.write_pos = 0;
        self.source_spans.clear();
        self.last_source_end = None;
        self.resource.get().clear_render();
        self.eof_seen = false;
        self.failed = false;
    }

    pub(super) const fn scratch_frames(sample_rate: u32) -> FrameCount {
        FrameCount::new(sample_rate as usize / Self::BUFFER_DURATION_DIVISOR)
    }

    /// Control-plane handle used to begin a seek off the audio thread.
    #[must_use]
    pub fn seek_handle(&self) -> Option<Arc<dyn kithara_audio::SeekBegin>> {
        self.resource.get().seek_handle()
    }

    delegate::delegate! {
        to self.resource.get() {
            /// Total duration in seconds. Returns 0.0 if unknown.
            #[must_use]
            #[expr($.map_or(0.0, |d| d.as_secs_f64()))]
            pub fn duration(&self) -> f64;
            /// Set the target sample rate of the audio host.
            pub(crate) fn set_host_sample_rate(&self, sample_rate: NonZeroU32);
            /// Update the scheduling priority hint for the shared worker.
            pub(crate) fn set_service_class(&self, class: ServiceClass);
            pub(crate) fn clear_render(&self);
            pub(crate) fn publish_render(
                &self,
                context: &RenderContext,
                frontier: PresentationFrontier,
            );
        }
    }
}
