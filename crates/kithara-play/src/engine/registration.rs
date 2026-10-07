use super::EngineImpl;
use crate::{error::PlayError, session::PlayerId};

impl<S> EngineImpl<S> {
    pub(in crate::engine) fn ensure_player_id(&self) -> Result<PlayerId, PlayError> {
        let mut registration = self.registration.lock();
        if let Some(id) = *registration {
            return Ok(id);
        }
        self.validate_session_sample_rate(self.session.requested_sample_rate()?.get())?;
        let id = self.session.register_player(
            self.config.grid_id,
            self.bus.clone(),
            self.pools().clone(),
            self.config.mixer,
        )?;
        *registration = Some(id);
        drop(registration);
        Ok(id)
    }

    pub(crate) fn prepare(&self) -> Result<(), PlayError> {
        if let Some(quantum) = self.config.render_quantum_frames
            && let Some(shape) = self.stream_shape()
        {
            shape.playback_buffers(quantum, self.config.response_budget_frames)?;
        }
        self.ensure_player_id().map(|_| ())
    }

    pub(super) fn registered_id(&self) -> Option<PlayerId> {
        *self.registration.lock()
    }
}
