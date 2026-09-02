use std::ops::{Deref, DerefMut};

use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_output::{
    OfflineRenderConfig, OfflineRenderError, OfflineRenderReport, OfflineRenderRequest,
    OfflineRenderer, RenderSink,
};
use kithara_platform::{CancelToken, sync::Arc};
use kithara_signal::AudioSpec;

use super::{Host, HostConfig, SessionRoot, platform::Platform};
use crate::{
    PlayError,
    session::{HostDispatcher, offline::OfflineSessionClient},
};

const CHANNELS: u16 = 2;

/// Canonical Host running its owned Firewheel graph without a device.
pub struct OfflineHost<S> {
    host: Host<S>,
    client: Arc<OfflineSessionClient<S>>,
    config: OfflineRenderConfig,
    position: u64,
    spec: AudioSpec,
}

impl<S> OfflineHost<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    /// Create a canonical Host with the native offline backend.
    ///
    /// # Errors
    /// Returns an error when a canonical Host identity cannot be allocated.
    pub fn new(
        config: HostConfig,
        render: OfflineRenderConfig,
        pools: PoolRegion<S>,
    ) -> Result<Self, PlayError> {
        let SessionRoot {
            id,
            sample_rate,
            group,
            view,
        } = Host::<S>::session_root(config)?;
        let client = crate::session::offline::spawn(
            group,
            view.clone(),
            sample_rate,
            render.block_frames(),
            pools,
        );
        let dispatcher: Arc<dyn HostDispatcher<S>> = client.clone();
        let host = Host::owner(id, view, dispatcher, Platform::owner());
        Ok(Self {
            host,
            client,
            config: render,
            position: 0,
            spec: AudioSpec::new(CHANNELS, sample_rate),
        })
    }

    /// Current absolute output-frame position.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Offline output format owned by this Host.
    #[must_use]
    pub const fn spec(&self) -> AudioSpec {
        self.spec
    }

    fn render_block(&mut self, frames: u64) -> Result<SampleBuffer, OfflineRenderError> {
        let frames = u32::try_from(frames).map_err(OfflineRenderError::backend)?;
        let block = self
            .client
            .render(frames)
            .map_err(OfflineRenderError::backend)?;
        self.position = self
            .position
            .checked_add(u64::from(frames))
            .ok_or_else(|| OfflineRenderError::backend(TimelineOverflow))?;
        Ok(block)
    }
}

impl<S> Deref for OfflineHost<S> {
    type Target = Host<S>;

    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

impl<S> DerefMut for OfflineHost<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.host
    }
}

impl<S> OfflineRenderer for OfflineHost<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn render(
        &mut self,
        request: &OfflineRenderRequest,
        cancel: &CancelToken,
        sink: &mut dyn RenderSink,
    ) -> Result<OfflineRenderReport, OfflineRenderError> {
        let requested_frames = request.frame_count()?;
        if request.spec() != self.spec {
            return Err(OfflineRenderError::SpecMismatch {
                expected: self.spec,
                actual: request.spec(),
            });
        }
        if request.frames().start < self.position {
            return Err(OfflineRenderError::RangeUnavailable {
                requested: request.frames().start,
                current: self.position,
            });
        }

        while self.position < request.frames().start {
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames: 0 });
            }
            let remaining = request.frames().start - self.position;
            let frames = remaining.min(u64::from(self.config.block_frames().get()));
            self.render_block(frames)?;
        }

        let mut rendered_frames = 0;
        while self.position < request.frames().end {
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames });
            }
            let remaining = request.frames().end - self.position;
            let frames = remaining.min(u64::from(self.config.block_frames().get()));
            let block = self.render_block(frames)?;
            if cancel.is_cancelled() {
                return Err(OfflineRenderError::Cancelled { rendered_frames });
            }
            sink.write(&block)
                .map_err(|error| OfflineRenderError::sink(rendered_frames, error))?;
            rendered_frames += frames;
        }
        if cancel.is_cancelled() {
            return Err(OfflineRenderError::Cancelled { rendered_frames });
        }
        debug_assert_eq!(rendered_frames, requested_frames);
        Ok(OfflineRenderReport::new(rendered_frames))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("offline Host timeline overflow")]
struct TimelineOverflow;
