use kithara_bufpool::HasPool;
use kithara_command::When;
use kithara_config::Configure;
use kithara_events::TrackId;
use kithara_play::{
    EffectChange, EffectConfigView, EffectSnapshot, EffectTarget, EqBandConfig, EqBandConfigChange,
    LaneId, PlayError,
};
use kithara_signal::SessionFrame;

use super::{QueueCommand, QueueControl};
use crate::QueueError;

/// Typed controls bound to one prepared effect in one loaded track.
#[derive_where::derive_where(Clone)]
#[non_exhaustive]
pub struct EffectControl<S, C: Copy>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    queue: QueueControl<S>,
    item: TrackId,
    lane: LaneId,
    target: EffectTarget,
    view: EffectConfigView<C>,
}

impl<S> QueueControl<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Bind to a band of the current track without changing its settings.
    /// # Errors
    /// Refuses an unloaded track or an absent effect or band.
    pub fn eq_band(
        &self,
        effect: usize,
        band: usize,
    ) -> Result<EffectControl<S, EqBandConfig>, QueueError> {
        let snapshot = self.view.read();
        let track = snapshot.track.as_ref().ok_or(PlayError::NotReady)?;
        let Some(EffectSnapshot::Equalizer { lane, bands }) = track.effects.get(effect) else {
            return Err(PlayError::InvalidConfiguration {
                reason: "equalizer is not installed".into(),
            }
            .into());
        };
        let view = bands
            .get(band)
            .ok_or(PlayError::EqBandOutOfRange {
                band,
                bands: bands.len(),
            })?
            .clone();
        Ok(EffectControl {
            queue: self.clone(),
            item: track.item,
            lane: *lane,
            target: EffectTarget::EqBand { effect, band },
            view,
        })
    }
}

impl<S> Configure<EqBandConfigChange> for EffectControl<S, EqBandConfig>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    type At = When<SessionFrame>;
    type Config = EqBandConfig;
    type Error = QueueError;
    type Output = ();

    fn configure(&self, change: EqBandConfigChange, at: Self::At) -> Result<(), QueueError> {
        self.queue.call(QueueCommand::ConfigureEffect {
            item: self.item,
            lane: self.lane,
            target: self.target,
            change: EffectChange::EqBand(change),
            at,
        })
    }

    fn settings(&self) -> EqBandConfig {
        self.view.config()
    }
}
