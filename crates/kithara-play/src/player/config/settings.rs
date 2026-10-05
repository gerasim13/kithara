use kithara_config::Config;
use kithara_render::LaneCommand;
use kithara_warp::{MIN_SPEED, SpeedCurve, StretchKind, WarpConfig};

use crate::PlayError;

/// What a track plays with that changes while it plays.
///
/// A change of one field goes to the track's render lane as one lane command;
/// the lane's receipt moves it into the settings the track's owner reads.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(check(error = PlayError), fields(value, get(copy)))]
pub(crate) struct TrackSettings {
    /// How fast the track plays, 1.0 at its own tempo.
    #[config(live, check = check_speed)]
    speed: f32,
    /// Whether the pitch stays put at any speed; only a backend with keylock
    /// keeps it.
    #[config(live)]
    keylock: bool,
    /// The time-stretch backend that renders the track.
    #[config(live)]
    backend: StretchKind,
}

impl TrackSettings {
    /// `base` for the renderer of a track that starts where these settings
    /// stand.
    pub(crate) fn warp(self, base: &WarpConfig) -> WarpConfig {
        base.starting_at(self.speed, self.keylock, self.backend)
    }
}

/// A speed is finite and no slower than the slowest speed the renderer plays.
fn check_speed(speed: f32) -> Result<f32, PlayError> {
    if speed.is_finite() && speed >= MIN_SPEED {
        Ok(speed)
    } else {
        Err(PlayError::InvalidParameter {
            name: "speed".to_owned(),
            value: speed,
        })
    }
}

impl From<TrackSettingsChange> for LaneCommand {
    fn from(change: TrackSettingsChange) -> Self {
        match change {
            TrackSettingsChange::Speed(speed) => Self::SetSpeed(SpeedCurve::Constant(speed)),
            TrackSettingsChange::Keylock(on) => Self::SetKeylock(on),
            TrackSettingsChange::Backend(kind) => Self::SetBackend(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_command::{ChannelConfig, Inbox, Live, LiveError, Sender, When, channel};
    use kithara_config::ConfigOwner;
    use kithara_render::{LaneCommand, LaneFrame, LaneProtocol};
    use kithara_test_utils::kithara;
    use kithara_warp::{SpeedCurve, StretchKind, WarpConfig};

    use super::{TrackSettings, TrackSettingsChange};
    use crate::PlayError;

    fn lane() -> (Sender<LaneProtocol>, Inbox<LaneProtocol>) {
        channel(ChannelConfig::builder().build())
    }

    fn settings() -> Live<TrackSettings, LaneProtocol> {
        Live::new(
            TrackSettings::builder()
                .speed(1.0)
                .keylock(false)
                .backend(StretchKind::default())
                .build(),
        )
        .expect("unity speed is a valid track speed")
    }

    /// Plays the lane at `frame`: applies every batch due there and returns
    /// their commands.
    fn execute(inbox: &mut Inbox<LaneProtocol>, frame: u64) -> Vec<LaneCommand> {
        inbox.drain();
        let mut commands = Vec::new();
        while let Some(due) = inbox.next_due(LaneFrame(frame), 1) {
            commands.extend(due.commands().iter().copied());
            due.apply(());
        }
        commands
    }

    #[kithara::test]
    #[case::next(When::Next, 0)]
    #[case::at_frame(When::At(LaneFrame(4_096)), 4_096)]
    fn a_change_shows_once_the_lane_applied_it(#[case] when: When<LaneFrame>, #[case] frame: u64) {
        let (mut sender, mut inbox) = lane();
        let mut live = settings();

        live.send(
            &mut sender,
            when,
            TrackSettingsChange::Keylock(true),
            LaneCommand::from,
        )
        .expect("the lane has room for one batch");
        assert!(
            !live.config().keylock(),
            "a sent change waits for its receipt"
        );

        let commands = execute(&mut inbox, frame);
        assert!(
            matches!(commands.as_slice(), [LaneCommand::SetKeylock(true)]),
            "the lane executes the change as its own command: {commands:?}"
        );
        for receipt in sender.receipts() {
            live.settle(&receipt);
        }
        assert!(live.config().keylock(), "the applied change shows");
    }

    #[kithara::test]
    fn every_change_reaches_the_lane_as_its_command() {
        assert!(matches!(
            LaneCommand::from(TrackSettingsChange::Speed(1.07)),
            LaneCommand::SetSpeed(SpeedCurve::Constant(speed)) if speed == 1.07
        ));
        assert!(matches!(
            LaneCommand::from(TrackSettingsChange::Keylock(true)),
            LaneCommand::SetKeylock(true)
        ));
        let backend = StretchKind::default();
        assert!(matches!(
            LaneCommand::from(TrackSettingsChange::Backend(backend)),
            LaneCommand::SetBackend(sent) if sent == backend
        ));
    }

    #[kithara::test]
    fn a_track_starts_with_live_warp_settings() {
        let quantum = NonZeroUsize::new(64).expect("fixture quantum is non-zero");
        let base = WarpConfig::builder()
            .speed(1.25)
            .render_quantum_frames(quantum)
            .build();
        let backend = StretchKind::default();
        let settings = TrackSettings::builder()
            .speed(0.8)
            .keylock(true)
            .backend(backend)
            .build();

        let warp = settings.warp(&base);

        assert!((warp.speed() - 0.8).abs() < f32::EPSILON);
        assert!(warp.keylock());
        assert_eq!(warp.backend(), backend);
        assert_eq!(warp.render_quantum_frames(), Some(quantum));
    }

    #[kithara::test]
    fn a_speed_under_the_floor_never_reaches_the_lane() {
        let (mut sender, mut inbox) = lane();
        let mut live = settings();

        let refused = live.send(
            &mut sender,
            When::Next,
            TrackSettingsChange::Speed(0.0),
            LaneCommand::from,
        );

        assert!(matches!(
            refused,
            Err(LiveError::Invalid(PlayError::InvalidParameter { .. }))
        ));
        assert!(execute(&mut inbox, 0).is_empty(), "nothing was sent");
        assert!((live.config().speed() - 1.0).abs() < f32::EPSILON);
    }
}
