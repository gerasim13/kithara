use kithara_config::Config;
use kithara_render::LaneCommand;
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_warp::StretchKind;
use kithara_warp::{MIN_SPEED, SpeedCurve, WarpConfig};

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
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[config(live)]
    keylock: bool,
    /// The time-stretch backend that renders the track.
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    #[config(live)]
    backend: StretchKind,
}

impl TrackSettings {
    /// `base` for the renderer of a track that starts where these settings
    /// stand. A build without a time-stretch backend renders every speed
    /// through `base` unchanged.
    pub(crate) fn warp(self, base: &WarpConfig) -> WarpConfig {
        #[cfg(any(
            feature = "stretch-signalsmith",
            feature = "stretch-bungee",
            feature = "stretch-glide"
        ))]
        {
            base.starting_at(self.speed, self.keylock, self.backend)
        }
        #[cfg(not(any(
            feature = "stretch-signalsmith",
            feature = "stretch-bungee",
            feature = "stretch-glide"
        )))]
        {
            base.clone()
        }
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
            #[cfg(any(
                feature = "stretch-signalsmith",
                feature = "stretch-bungee",
                feature = "stretch-glide"
            ))]
            TrackSettingsChange::Keylock(on) => Self::SetKeylock(on),
            #[cfg(any(
                feature = "stretch-signalsmith",
                feature = "stretch-bungee",
                feature = "stretch-glide"
            ))]
            TrackSettingsChange::Backend(kind) => Self::SetBackend(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_command::{ChannelConfig, Inbox, Live, LiveError, Sender, When, channel};
    use kithara_config::ConfigOwner;
    use kithara_render::{LaneCommand, LaneFrame, LaneProtocol};
    use kithara_test_utils::kithara;
    use kithara_warp::{SpeedCurve, StretchKind};

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
