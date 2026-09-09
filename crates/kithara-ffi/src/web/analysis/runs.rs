use std::{cell::RefCell, collections::HashMap, num::NonZeroU32, rc::Rc};

use kithara::{
    analysis::{
        AnalysisProgress, AnalysisToken, AnalysisWorker, AnalysisWorkerConfig, AnalyzerBuilder,
        BeatAnalysisConfig,
    },
    audio::AudioReader,
    platform::{
        CancelToken,
        sync::Arc,
        tokio::{self, sync::watch, task::spawn as task_spawn},
    },
    prelude::{PlaybackResamplerBackend, Resource},
    queue::TrackId,
};
use web_sys::BroadcastChannel;

use super::encode::encode;
use crate::{
    pools::{FfiPools, FfiQueueControl, FfiResourceConfig, Pools},
    web::{interop::send_reply, observer::source::EVENT_CHANNEL},
};

struct Consts;

impl Consts {
    const CALLER_HOLDS_NO_REVISION: u64 = 0;
    const WAVEFORM_MAX_BUCKETS: usize = 96_000;
}

type WebAnalyzerBuilder = AnalyzerBuilder<PlaybackResamplerBackend, FfiPools>;

type Live = Rc<RefCell<HashMap<TrackId, CancelToken>>>;

/// The engine worker's analysis owner: one shared [`AnalysisWorker`] and the
/// cancel token of the single live pass per track.
pub(crate) struct AnalysisRuns {
    worker: Arc<AnalysisWorker>,
    live: Live,
}

impl AnalysisRuns {
    pub(crate) fn new(pools: Pools) -> Self {
        let builder: WebAnalyzerBuilder = AnalyzerBuilder::new(pools)
            .with_beat_config(BeatAnalysisConfig::default())
            .with_waveform(Consts::WAVEFORM_MAX_BUCKETS)
            .with_beat();
        Self {
            worker: Arc::new(AnalysisWorker::new(
                AnalysisWorkerConfig::for_builder(builder).build(),
            )),
            live: Live::default(),
        }
    }

    pub(crate) fn cancel(&mut self, id: TrackId) {
        if let Some(cancel) = self.live.borrow_mut().remove(&id) {
            cancel.cancel();
        }
    }

    pub(crate) fn clear(&mut self) {
        for (_, cancel) in self.live.borrow_mut().drain() {
            cancel.cancel();
        }
    }

    /// Open a pass for `id` on the queue's decoded-audio axis.
    pub(crate) fn start(
        &mut self,
        queue: &FfiQueueControl,
        config: FfiResourceConfig,
        id: TrackId,
        token: AnalysisToken,
        request_id: u32,
    ) {
        let Some(rate) = NonZeroU32::new(queue.sample_rate()) else {
            send_reply(
                request_id,
                Err("the queue reports no sample rate".to_owned()),
            );
            return;
        };
        self.cancel(id);

        let (rx, producer, pass) = self
            .worker
            .open(token, rate, Consts::CALLER_HOLDS_NO_REVISION);
        let cancel = pass.cancel_token().clone();
        self.live.borrow_mut().insert(id, cancel.clone());

        let worker = Arc::clone(&self.worker);
        let reader_cancel = cancel.clone();
        let queue = queue.clone();
        task_spawn(async move {
            match open_reader(config, &reader_cancel, rate).await {
                Ok(reader) => {
                    queue.attach_observer(id, producer);
                    worker.start(pass, reader);
                    send_reply(request_id, Ok(()));
                }
                Err(error) => {
                    tracing::warn!(?error, "analysis: reader did not open; pass never started");
                    send_reply(request_id, Err(error));
                }
            }
        });
        let live = Rc::clone(&self.live);
        task_spawn(async move {
            publish(id, rx, &cancel).await;
            if !cancel.is_cancelled() {
                live.borrow_mut().remove(&id);
            }
        });
    }
}

async fn publish(
    id: TrackId,
    mut rx: watch::Receiver<Option<AnalysisProgress>>,
    cancel: &CancelToken,
) {
    let Ok(channel) = BroadcastChannel::new(EVENT_CHANNEL) else {
        tracing::warn!("analysis: BroadcastChannel unavailable in worker");
        return;
    };
    loop {
        let changed = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            changed = rx.changed() => changed,
        };
        if changed.is_err() {
            return;
        }
        let message = rx
            .borrow_and_update()
            .as_ref()
            .map(|progress| encode(id, progress.analysis()));
        if let Some(message) = message {
            let _ = channel.post_message(&message);
        }
    }
}

async fn open_reader(
    mut config: FfiResourceConfig,
    cancel: &CancelToken,
    rate: NonZeroU32,
) -> Result<Box<dyn AudioReader>, String> {
    if cancel.is_cancelled() {
        return Err("the pass was cancelled before its reader opened".to_owned());
    }
    config.set_cancel(cancel.child());
    config.set_host_sample_rate(rate);
    let mut resource = Resource::new(config)
        .await
        .map_err(|error| format!("resource open failed: {error}"))?;
    resource
        .preload()
        .await
        .map_err(|error| format!("preload failed: {error}"))?;
    Ok(resource.into())
}
