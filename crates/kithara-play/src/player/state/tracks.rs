//! The tracks a player's deck holds.

use kithara_command::{Sender, Seq, When};
use kithara_config::LiveConfig;
use kithara_events::TrackId;
use kithara_render::{LaneProtocol, rt::track::PlayerResource};

use crate::{
    PlayError,
    player::{
        config::{TrackSettings, TrackSettingsChange},
        track::{Behind, Outbox, Player, Track, TrackCommand},
    },
};

/// Every track the deck holds, oldest first, and the settings the next track
/// it takes starts with.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(crate) struct Tracks {
    /// The settings the next track the deck takes starts with.
    #[field(get, copy, vis = "pub(crate)")]
    next: TrackSettings,
    held: Vec<Track>,
}

impl Tracks {
    /// A deck that holds no track yet; the first it takes starts with `next`.
    pub(crate) const fn new(next: TrackSettings) -> Self {
        Self {
            next,
            held: Vec::new(),
        }
    }

    /// Puts `item_id` on the deck, chained `behind` a track when one is named,
    /// as a track that starts with the next-track settings and reads `lane`.
    /// The track is held once the deck admits it.
    ///
    /// # Errors
    ///
    /// Returns the deck's refusal; the resource is spent then.
    pub(crate) fn load(
        &mut self,
        item_id: TrackId,
        resource: Box<PlayerResource>,
        lane: Option<Sender<LaneProtocol>>,
        behind: Option<Behind>,
        out: &mut Outbox<'_>,
    ) -> Result<(), PlayError> {
        let mut track = Track::new(item_id, self.next)?;
        track.apply(
            TrackCommand::Load {
                resource,
                lane,
                behind,
            },
            out,
        )?;
        self.held.push(track);
        Ok(())
    }

    /// Tells the newest held track `item_id` to do `command`. A track the
    /// deck no longer holds sends nothing.
    ///
    /// # Errors
    ///
    /// Returns the deck's refusal of the batch the command became.
    pub(crate) fn apply(
        &mut self,
        item_id: TrackId,
        command: TrackCommand,
        out: &mut Outbox<'_>,
    ) -> Result<Option<Seq>, PlayError> {
        self.held
            .iter_mut()
            .rev()
            .find(|track| track.item_id() == item_id)
            .map_or(Ok(None), |track| track.apply(command, out))
    }

    /// Makes `speed` the next tracks' speed and returns the change that sets
    /// it.
    ///
    /// # Errors
    ///
    /// Returns the speed check's refusal; nothing changes then.
    pub(crate) fn set_next_speed(&mut self, speed: f32) -> Result<TrackSettingsChange, PlayError> {
        let change = TrackSettings::check(TrackSettingsChange::Speed(speed))?;
        self.next.apply_change(change);
        Ok(change)
    }

    /// Sends `change` to every held track's lane for its next block and hands
    /// `sent` the number of each batch that went out.
    ///
    /// # Errors
    ///
    /// Returns the change's check refusal.
    pub(crate) fn configure(
        &mut self,
        change: TrackSettingsChange,
        out: &mut Outbox<'_>,
        mut sent: impl FnMut(Seq),
    ) -> Result<(), PlayError> {
        for track in &mut self.held {
            if let Some(seq) = track.apply(TrackCommand::Configure(change, When::Next), out)? {
                sent(seq);
            }
        }
        Ok(())
    }

    /// Releases the oldest held track `item_id`, one the deck unloaded.
    pub(crate) fn unload(&mut self, item_id: TrackId) {
        if let Some(index) = self
            .held
            .iter()
            .position(|track| track.item_id() == item_id)
        {
            self.held.remove(index);
        }
    }
}
