use std::num::NonZeroUsize;

use kithara_command::Target;

use super::track::PlayerTrack;
use crate::bridge::Slot;

/// The tracks a mixer holds, one per slot its owner assigns.
pub(crate) struct TrackSlots {
    slots: Box<[Option<PlayerTrack>]>,
}

impl TrackSlots {
    /// `capacity` empty slots, allocated once: a mixer never grows them on the audio thread.
    pub(crate) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            slots: (0..capacity.get()).map(|_| None).collect(),
        }
    }

    /// The track in `slot`; `None` for an empty slot or one past the mixer's count.
    pub(crate) fn at(&self, slot: Slot) -> Option<&PlayerTrack> {
        self.slots.get(slot.index())?.as_ref()
    }

    pub(crate) fn at_mut(&mut self, slot: Slot) -> Option<&mut PlayerTrack> {
        self.slots.get_mut(slot.index())?.as_mut()
    }

    /// Put `track` into `slot`, handing back the track it held.
    pub(crate) fn put(&mut self, slot: Slot, track: PlayerTrack) -> Option<PlayerTrack> {
        self.slots.get_mut(slot.index())?.replace(track)
    }

    /// Take the track out of `slot`.
    pub(crate) fn take(&mut self, slot: Slot) -> Option<PlayerTrack> {
        self.slots.get_mut(slot.index())?.take()
    }

    /// Whether `slot` holds a track.
    pub(crate) fn is_held(&self, slot: Slot) -> bool {
        self.at(slot).is_some()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (Slot, &mut PlayerTrack)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, held)| Some((slot(index), held.as_mut()?)))
    }

    /// Every slot, held or empty, in order.
    pub(crate) fn slots(&self) -> impl Iterator<Item = Slot> + use<> {
        (0..self.slots.len()).map(slot)
    }

    pub(crate) fn count(&self) -> usize {
        self.slots.len()
    }
}

/// The slot at `index`; a mixer never holds more slots than a `Slot` counts.
fn slot(index: usize) -> Slot {
    Slot::new(u16::try_from(index).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::mock::{AudioReadMock, AudioSessionMock};
    use kithara_platform::{sync::Arc, time::Duration};
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        rt::track::{PcmConsumer, PlayerResource},
        test_pools::pools,
    };

    fn track(src: Arc<str>) -> PlayerTrack {
        let sample_rate = NonZeroU32::new(44_100).expect("static sample rate");
        let reader = Unimock::new((
            AudioSessionMock::duration
                .each_call(matching!())
                .returns(Some(Duration::from_secs(1))),
            AudioReadMock::spec
                .each_call(matching!())
                .returns(AudioSpec::new(2, sample_rate)),
        ));
        let resource = PlayerResource::new(PcmConsumer::new(Box::new(reader)), src, &pools())
            .map_or_else(|error| panic!("test player resource: {error}"), Box::new);

        PlayerTrack::builder()
            .sample_rate(sample_rate)
            .build(resource)
    }

    #[kithara::test]
    fn identical_sources_are_addressed_by_slot() {
        let src: Arc<str> = Arc::from("same.mp3");
        let (first, second) = (Slot::new(0), Slot::new(1));
        let mut tracks = TrackSlots::new(NonZeroUsize::new(2).expect("two slots"));

        assert!(tracks.put(first, track(Arc::clone(&src))).is_none());
        assert!(tracks.put(second, track(src)).is_none());
        assert!(tracks.is_held(first) && tracks.is_held(second));

        assert!(tracks.take(first).is_some());
        assert!(!tracks.is_held(first));
        assert!(tracks.is_held(second));
        assert!(tracks.at(Slot::new(2)).is_none(), "past the mixer's slots");
    }
}
