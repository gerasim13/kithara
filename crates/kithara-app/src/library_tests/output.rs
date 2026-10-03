use std::{io, num::NonZeroU32};

use ::kithara::{
    audio::AudioEvent,
    output::{OfflineRenderRequest, OfflineRenderer, RenderSink, RenderSinkError},
    platform::time::{Duration, Instant},
    queue::{QueueEvent, TrackStatus, Transition},
    signal::AudioSpec,
};
use serde_json::{Value, json};

use crate::{analysis::fixtures::queue_off, config::AppConfig, sources::build_source};

#[derive(Default)]
struct Output {
    samples: u64,
    nonzero: u64,
    peak: f32,
}

impl RenderSink for Output {
    fn write(&mut self, samples: &[f32]) -> Result<(), RenderSinkError> {
        for &sample in samples {
            if !sample.is_finite() {
                return Err(RenderSinkError::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "nonfinite Host output",
                )));
            }
            self.peak = self.peak.max(sample.abs());
            self.nonzero += u64::from(sample != 0.0);
        }
        self.samples += samples.len() as u64;
        Ok(())
    }
}

pub(super) async fn play_source(path: &str, config: &AppConfig) -> Result<Value, String> {
    let (host, queue) = queue_off().await;
    let mut audio = queue.subscribe::<AudioEvent>();
    let mut events = queue.subscribe::<QueueEvent>();
    let source = build_source(path, config);
    let cancel = config.shutdown.child();
    let result = async {
        let id = host.call(move |(_, queue)| {
            let id = queue.append(source)?;
            queue.select(id, Transition::None)?;
            queue.play();
            Ok::<_, ::kithara::queue::QueueError>(id)
        }).await.map_err(|error| error.to_string())?;
        let loading = Instant::now();
        loop {
            let status = queue.track(id).ok_or("track disappeared while loading")?.status;
            match status {
                TrackStatus::Loaded | TrackStatus::Consumed => break,
                TrackStatus::Failed(reason) => return Err(reason),
                TrackStatus::Cancelled => return Err("track cancelled while loading".to_owned()),
                _ => {}
            }
            if loading.elapsed() > Duration::from_secs(30) {
                return Err("track loading exceeded 30 seconds".to_owned());
            }
            ::kithara::platform::time::sleep(Duration::from_millis(1)).await;
        }
        let rate = NonZeroU32::new(queue.sample_rate()).ok_or("zero output rate")?;
        let spec = AudioSpec::new(2, rate);
        let mut cursor = 0;
        let mut output = Output::default();
        let mut decoded_eof = false;
        let mut ended = false;
        let started = Instant::now();
        loop {
            let token = cancel.child();
            let (next, samples) = host.call(move |(host, queue)| {
                queue.tick().map_err(|error| error.to_string())?;
                let end = cursor + 16_384;
                let request = OfflineRenderRequest::builder().spec(spec).frames(cursor..end).build();
                let mut samples = Output::default();
                host.render(&request, &token, &mut samples).map_err(|error| error.to_string())?;
                queue.tick().map_err(|error| error.to_string())?;
                Ok::<_, String>((end, samples))
            }).await?;
            cursor = next;
            output.samples += samples.samples;
            output.nonzero += samples.nonzero;
            output.peak = output.peak.max(samples.peak);
            while let Ok(envelope) = audio.try_recv() {
                if envelope.meta.track != Some(id) {
                    continue;
                }
                match envelope.event {
                    AudioEvent::EndOfStream { .. } => decoded_eof = true,
                    AudioEvent::TrackFailed { failure, .. } => return Err(format!("audio failed: {failure:?}")),
                    _ => {}
                }
            }
            while let Ok(envelope) = events.try_recv() {
                match envelope.event {
                    QueueEvent::QueueEnded => ended = true,
                    QueueEvent::TrackLoadFailed { id: failed, reason, .. } if failed == id => return Err(reason),
                    _ => {}
                }
            }
            if ended {
                if !decoded_eof || output.nonzero == 0 {
                    return Err(format!("ended without decoded EOF or audible output: eof={decoded_eof}, nonzero={}", output.nonzero));
                }
                return Ok(json!({"frames": output.samples / 2, "samples": output.samples,
                    "nonzero": output.nonzero, "peak": output.peak, "sample_rate": rate.get(),
                    "channels": 2, "decoded_eof": decoded_eof, "queue_ended": ended,
                    "position_seconds": queue.position_seconds()}));
            }
            if started.elapsed() > Duration::from_secs(600) {
                return Err("Host playback exceeded 600 seconds".to_owned());
            }
        }
    }.await;
    cancel.cancel();
    host.call(|(_, queue)| queue.clear()).await;
    drop(queue);
    host.close().await;
    result
}
