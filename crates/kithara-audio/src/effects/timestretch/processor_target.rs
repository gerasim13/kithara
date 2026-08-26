use kithara_bufpool::{PcmBuf, PcmPool};
use kithara_decode::PcmSpec;
use kithara_stretch::{ElasticConfig, ElasticEngine, ElasticError, StretchKind, build_engine};
use tracing::warn;

use super::processor::TimeStretchProcessor;

#[derive(Default)]
pub(super) struct PreparedTarget {
    pub(super) engine: Option<Box<dyn ElasticEngine>>,
    pub(super) pending_source: Option<PcmBuf>,
    pub(super) scratch: Option<PcmBuf>,
}

impl TimeStretchProcessor {
    pub(super) fn prepare_target(
        kind: StretchKind,
        spec: PcmSpec,
        pool: &PcmPool,
        reusable_pending: Option<PcmBuf>,
        reusable_scratch: Option<PcmBuf>,
    ) -> PreparedTarget {
        let result = Self::config_for(kind, spec, pool)
            .and_then(build_engine)
            .and_then(|engine| {
                let channels = usize::from(spec.channels.max(1));
                let pending_samples = Self::MAX_SOURCE_FRAMES
                    .checked_mul(channels)
                    .ok_or(ElasticError::SampleCountOverflow)?;
                let mut pending = reusable_pending.unwrap_or_else(|| pool.get());
                pending
                    .ensure_len(pending_samples)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                pending.clear();
                let scratch_samples = Self::scratch_samples(engine.as_ref(), spec)?;
                let mut scratch = reusable_scratch.unwrap_or_else(|| pool.get());
                scratch
                    .ensure_len(scratch_samples)
                    .map_err(|_| ElasticError::PcmPoolBudgetExhausted)?;
                scratch.clear();
                Ok((engine, pending, scratch))
            });
        match result {
            Ok((engine, pending, scratch)) => PreparedTarget {
                engine: Some(engine),
                pending_source: Some(pending),
                scratch: Some(scratch),
            },
            Err(error) => {
                warn!(%kind, %error, "time-stretch engine preparation failed");
                PreparedTarget::default()
            }
        }
    }

    fn config_for(
        backend: StretchKind,
        spec: PcmSpec,
        pool: &PcmPool,
    ) -> Result<ElasticConfig, ElasticError> {
        ElasticConfig::builder()
            .backend(backend)
            .sample_rate(spec.sample_rate.get())
            .channels(usize::from(spec.channels.max(1)))
            .pool(pool.clone())
            .max_source_frames(Self::MAX_SOURCE_FRAMES)
            .max_output_frames(Self::MAX_OUTPUT_FRAMES)
            .build()
    }

    fn scratch_samples(engine: &dyn ElasticEngine, spec: PcmSpec) -> Result<usize, ElasticError> {
        let capabilities = engine.capabilities();
        capabilities
            .max_output_frames()
            .max(capabilities.terminal_chunk_frames().saturating_add(1))
            .checked_mul(usize::from(spec.channels.max(1)))
            .ok_or(ElasticError::SampleCountOverflow)
    }

    fn service_scratch(&mut self) {
        if self.scratch.is_some() {
            drop(self.deferred_scratch.take());
            return;
        }
        let Some(engine) = self.engine.as_deref() else {
            drop(self.deferred_scratch.take());
            return;
        };
        let required = match Self::scratch_samples(engine, self.spec) {
            Ok(required) => required,
            Err(error) => {
                warn!(%error, "time-stretch output scratch sizing failed");
                drop(self.deferred_scratch.take());
                return;
            }
        };
        let mut scratch = self
            .deferred_scratch
            .take()
            .unwrap_or_else(|| self.pool.get());
        if scratch.ensure_len(required).is_err() {
            warn!("PCM pool budget exhausted while preparing time-stretch output scratch");
            return;
        }
        scratch.clear();
        self.scratch = Some(scratch);
    }

    /// Service backend/spec changes and deferred destruction from the
    /// scheduler shell, never from the checked render core.
    pub(super) fn service_target(&mut self, spec: PcmSpec) {
        drop(self.retired_engine.take());
        self.sync_plan();

        let kind = self.controls.backend();
        if kind != self.current_kind || spec != self.spec {
            drop(self.deferred_scratch.take());
            self.clear_render_state();
            let reusable_pending = self.pending_source.take();
            let reusable_scratch = self.scratch.take();
            drop(self.engine.take());
            let target =
                Self::prepare_target(kind, spec, &self.pool, reusable_pending, reusable_scratch);
            self.engine = target.engine;
            self.pending_source = target.pending_source;
            self.scratch = target.scratch;
            self.current_kind = kind;
            self.spec = spec;
            self.reset_pending = false;
            return;
        }

        self.service_scratch();

        if !self.reset_pending {
            return;
        }
        self.reset_pending = false;
        if let Some(engine) = self.engine.as_mut()
            && let Err(error) = engine.reset()
        {
            warn!(%error, "time-stretch deferred reset failed");
            self.engine = None;
        }
    }
}
