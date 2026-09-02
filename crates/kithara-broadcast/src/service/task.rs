use arc_swap::ArcSwap;
use kithara_bufpool::SampleBuffer;
use kithara_encode::{StreamBackend, StreamEncoder};
use kithara_platform::sync::{Arc, atomic::Ordering, mpsc::Sender};
use kithara_worker::{Task, TickResult};
use ringbuf::{
    HeapCons,
    traits::{Consumer, Observer},
};

use super::{Control, Counters};
use crate::{
    BroadcastResult,
    config::BroadcastConfig,
    segment::{Segment, Segmenter},
    server::{self, Origin},
    window::LiveWindow,
};

pub(super) struct BroadcastTask {
    completed: Option<Sender<()>>,
    control: Arc<Control>,
    counted_drops: u64,
    counters: Arc<Counters>,
    encoder: Option<StreamEncoder>,
    origin: Arc<Origin>,
    pcm: HeapCons<f32>,
    scratch: Option<SampleBuffer>,
    segmenter: Segmenter,
    window: LiveWindow,
}

impl BroadcastTask {
    pub(super) fn new(
        config: &BroadcastConfig,
        pcm: HeapCons<f32>,
        control: Arc<Control>,
        scratch: SampleBuffer,
        completed: Sender<()>,
    ) -> BroadcastResult<Self> {
        let window = LiveWindow::new(config)?;
        Ok(Self {
            completed: Some(completed),
            control,
            counted_drops: 0,
            counters: Arc::new(Counters::default()),
            encoder: Some(
                StreamEncoder::builder()
                    .backend(StreamBackend::Fdk)
                    .sample_rate(config.sample_rate)
                    .channels(config.channels)
                    .bit_rate(config.bit_rate)
                    .timescale(config.sample_rate)
                    .build()?,
            ),
            origin: Arc::new(Origin {
                snapshot: ArcSwap::from_pointee(window.snapshot()),
                master: Arc::from(server::master_playlist(config.bit_rate)),
            }),
            pcm,
            scratch: Some(scratch),
            segmenter: Segmenter::new(config)?,
            window,
        })
    }

    pub(super) fn counters(&self) -> Arc<Counters> {
        Arc::clone(&self.counters)
    }

    pub(super) fn origin(&self) -> Arc<Origin> {
        Arc::clone(&self.origin)
    }

    fn complete(&mut self) {
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(());
        }
    }

    fn finish(&mut self) -> BroadcastResult<()> {
        if let Some(encoder) = self.encoder.take() {
            for unit in encoder.finish()? {
                if let Some(segment) = self.segmenter.push(&unit)? {
                    self.publish(segment);
                }
            }
        }
        if let Some(segment) = self.segmenter.flush() {
            self.publish(segment);
        }
        self.window.finish();
        self.publish_snapshot();
        Ok(())
    }

    fn mark_drops(&mut self) {
        let total = self.control.dropped();
        let dropped = total.saturating_sub(self.counted_drops);
        self.counted_drops = total;
        if dropped == 0 {
            return;
        }
        self.counters.dropped.fetch_add(dropped, Ordering::Relaxed);
        if let Some(segment) = self.segmenter.mark_drop() {
            self.publish(segment);
        }
    }

    fn process(&mut self, samples: &[f32]) -> BroadcastResult<()> {
        if let Some(encoder) = self.encoder.as_mut()
            && !samples.is_empty()
        {
            for unit in encoder.push(samples)? {
                if let Some(segment) = self.segmenter.push(&unit)? {
                    self.publish(segment);
                }
            }
        }
        Ok(())
    }

    fn publish(&mut self, segment: Segment) {
        self.window.push(segment);
        self.counters.segments.fetch_add(1, Ordering::Relaxed);
        self.publish_snapshot();
    }

    fn publish_snapshot(&self) {
        self.origin.snapshot.store(Arc::new(self.window.snapshot()));
    }

    fn fail(&mut self, error: &dyn std::fmt::Display) -> TickResult {
        tracing::error!(%error, "the live packager stopped");
        self.control.finish();
        self.encoder.take();
        if let Some(segment) = self.segmenter.flush() {
            self.publish(segment);
        }
        self.window.finish();
        self.publish_snapshot();
        self.complete();
        TickResult::Done
    }
}

impl Task for BroadcastTask {
    fn on_cancel(&mut self) {
        self.encoder.take();
        self.complete();
    }

    fn tick(&mut self) -> TickResult {
        let Some(mut scratch) = self.scratch.take() else {
            return TickResult::Done;
        };
        let samples = self.pcm.occupied_len().min(scratch.len());
        let samples = samples - samples % 2;
        let taken = self.pcm.pop_slice(&mut scratch[..samples]);
        let processed = self.process(&scratch[..taken]);
        self.scratch = Some(scratch);
        if let Err(error) = processed {
            return self.fail(&error);
        }
        self.mark_drops();

        if self.control.is_finished(self.pcm.is_empty()) {
            if let Err(error) = self.finish() {
                return self.fail(&error);
            }
            self.complete();
            return TickResult::Done;
        }
        if taken > 0 {
            TickResult::Progress
        } else {
            TickResult::Waiting
        }
    }
}
