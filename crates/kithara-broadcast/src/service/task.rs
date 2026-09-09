use arc_swap::ArcSwap;
use kithara_bufpool::SampleBuffer;
use kithara_encode::{StreamBackend, StreamEncoder};
use kithara_platform::sync::{Arc, atomic::Ordering, mpsc::Sender};
use kithara_worker::{Task, TickResult};
use ringbuf::{
    HeapCons,
    traits::{Consumer, Observer},
};

use super::{Control, Counters, FormatChange};
use crate::{
    BroadcastResult,
    config::BroadcastConfig,
    segment::{Segment, Segmenter},
    server::{self, Origin},
    window::LiveWindow,
};

pub(super) struct BroadcastTask<S> {
    control: Arc<Control>,
    counters: Arc<Counters>,
    origin: Arc<Origin>,
    config: BroadcastConfig<S>,
    formats: HeapCons<FormatChange>,
    pcm: HeapCons<f32>,
    window: LiveWindow,
    completed: Option<Sender<()>>,
    encoder: Option<StreamEncoder>,
    next_format: Option<FormatChange>,
    scratch: Option<SampleBuffer>,
    segmenter: Segmenter,
    counted_drops: u64,
    frames: u64,
    generation_capacity: usize,
}

impl<S> BroadcastTask<S>
where
    S: Send + Sync + 'static,
{
    pub(super) fn new(
        config: BroadcastConfig<S>,
        pcm: HeapCons<f32>,
        formats: HeapCons<FormatChange>,
        control: Arc<Control>,
        scratch: SampleBuffer,
        completed: Sender<()>,
    ) -> BroadcastResult<Self> {
        let window = LiveWindow::new(&config)?;
        let encoder = Self::open_encoder(&config)?;
        let segmenter = Segmenter::new(&config)?;
        let bit_rate = config.bit_rate;
        let generation_capacity = config.generation_capacity.get();
        Ok(Self {
            completed: Some(completed),
            config,
            control,
            counted_drops: 0,
            counters: Arc::new(Counters::default()),
            encoder: Some(encoder),
            formats,
            frames: 0,
            generation_capacity,
            next_format: None,
            origin: Arc::new(Origin {
                snapshot: ArcSwap::from_pointee(window.snapshot()),
                master: Arc::from(server::master_playlist(bit_rate)),
            }),
            pcm,
            scratch: Some(scratch),
            segmenter,
            window,
        })
    }

    fn complete(&mut self) {
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(());
        }
    }

    pub(super) fn counters(&self) -> Arc<Counters> {
        Arc::clone(&self.counters)
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

    fn finish(&mut self) -> BroadcastResult<()> {
        self.finish_encoder()?;
        if let Some(segment) = self.segmenter.flush() {
            self.publish(segment);
        }
        self.window.finish();
        self.publish_snapshot();
        Ok(())
    }

    fn finish_encoder(&mut self) -> BroadcastResult<()> {
        if let Some(encoder) = self.encoder.take() {
            for unit in encoder.finish()? {
                if let Some(segment) = self.segmenter.push(&unit)? {
                    self.publish(segment);
                }
            }
        }
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

    fn next_format(&mut self) -> Option<FormatChange> {
        if self.next_format.is_none() {
            self.next_format = self.formats.try_pop();
        }
        self.next_format
    }

    fn open_encoder(config: &BroadcastConfig<S>) -> BroadcastResult<StreamEncoder> {
        Ok(StreamEncoder::builder()
            .backend(StreamBackend::Fdk)
            .sample_rate(config.sample_rate)
            .channels(config.channels)
            .bit_rate(config.bit_rate)
            .timescale(config.sample_rate)
            .build()?)
    }

    pub(super) fn origin(&self) -> Arc<Origin> {
        Arc::clone(&self.origin)
    }

    fn process(&mut self, samples: &[f32]) -> BroadcastResult<()> {
        let mut sample = 0;
        loop {
            if let Some(change) = self.next_format()
                && change.frame <= self.frames
            {
                self.reconfigure(change)?;
                continue;
            }
            if sample >= samples.len() {
                break;
            }
            let available = (samples.len() - sample) / 2;
            let mut take =
                u64::try_from(available).map_err(|_| crate::BroadcastError::CapacityOverflow)?;
            if let Some(change) = self.next_format() {
                take = take.min(change.frame - self.frames);
            }
            let take =
                usize::try_from(take).map_err(|_| crate::BroadcastError::CapacityOverflow)?;
            let end = sample
                .checked_add(
                    take.checked_mul(2)
                        .ok_or(crate::BroadcastError::CapacityOverflow)?,
                )
                .ok_or(crate::BroadcastError::CapacityOverflow)?;
            if let Some(encoder) = self.encoder.as_mut() {
                for unit in encoder.push(&samples[sample..end])? {
                    if let Some(segment) = self.segmenter.push(&unit)? {
                        self.publish(segment);
                    }
                }
            }
            self.frames = self
                .frames
                .checked_add(
                    u64::try_from(take).map_err(|_| crate::BroadcastError::CapacityOverflow)?,
                )
                .ok_or(crate::BroadcastError::CapacityOverflow)?;
            sample = end;
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

    fn reconfigure(&mut self, change: FormatChange) -> BroadcastResult<()> {
        if change.spec.channels != self.config.channels {
            return Err(crate::BroadcastError::LiveChannelCount {
                channels: change.spec.channels,
            });
        }
        self.finish_encoder()?;
        let config = self.config.with_sample_rate(change.spec.sample_rate.get());
        if let Some(segment) = self.segmenter.reconfigure(&config)? {
            self.publish(segment);
        }
        self.encoder = Some(Self::open_encoder(&config)?);
        self.config = config;
        self.next_format = None;
        Ok(())
    }
}

impl<S> Task for BroadcastTask<S>
where
    S: Send + Sync + 'static,
{
    fn on_cancel(&mut self) {
        self.encoder.take();
        self.complete();
    }

    fn tick(&mut self) -> TickResult {
        if self.control.generation_overflowed.load(Ordering::Acquire) {
            return self.fail(&crate::BroadcastError::GenerationQueueOverflow {
                capacity: self.generation_capacity,
            });
        }
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
