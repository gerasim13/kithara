use kithara_events::TrackId;

use super::track::PlayerTrack;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrackSlot(usize);

pub(crate) struct TrackSlots<const CAPACITY: usize> {
    slots: [Option<PlayerTrack>; CAPACITY],
}

impl<const CAPACITY: usize> Default for TrackSlots<CAPACITY> {
    fn default() -> Self {
        Self {
            slots: [const { None }; CAPACITY],
        }
    }
}

impl<const CAPACITY: usize> TrackSlots<CAPACITY> {
    pub(crate) fn get(&self, item_id: TrackId) -> Option<&PlayerTrack> {
        self.iter()
            .find_map(|(_, track)| (track.item_id() == item_id).then_some(track))
    }

    pub(crate) fn get_mut(&mut self, item_id: TrackId) -> Option<&mut PlayerTrack> {
        self.iter_mut()
            .find_map(|(_, track)| (track.item_id() == item_id).then_some(track))
    }

    /// Place a track in a free slot, handing it back when every slot is taken — a `PlayerTrack`
    /// must not be freed on the audio thread.
    pub(crate) fn insert(&mut self, track: PlayerTrack) -> Option<PlayerTrack> {
        match self.slots.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(track);
                None
            }
            None => Some(track),
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (TrackSlot, &PlayerTrack)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(idx, slot)| Some((TrackSlot(idx), slot.as_ref()?)))
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (TrackSlot, &mut PlayerTrack)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, slot)| Some((TrackSlot(idx), slot.as_mut()?)))
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub(crate) fn slot_of(&self, item_id: TrackId) -> Option<TrackSlot> {
        self.iter()
            .find_map(|(slot, track)| (track.item_id() == item_id).then_some(slot))
    }

    delegate::delegate! {
        to self.slots {
            #[expr($?.as_mut())]
            #[call(get_mut)]
            pub(crate) fn at_mut(&mut self, #[newtype] slot: TrackSlot) -> Option<&mut PlayerTrack>;
            #[expr($.all(Option::is_some))]
            #[call(iter)]
            pub(crate) fn is_full(&self) -> bool;
            #[expr($?.take())]
            #[call(get_mut)]
            pub(crate) fn remove_at(&mut self, #[newtype] slot: TrackSlot) -> Option<PlayerTrack>;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::{
        ReadOutcome, SeekBegin,
        mock::{AudioControlMock, AudioReadMock, AudioSessionMock},
    };
    use kithara_decode::DecodeError;
    use kithara_events::EventBus;
    use kithara_platform::{sync::Arc, time::Duration};
    use kithara_play::Resource;
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;
    use kithara_warp::{PresentationFrontier, RenderContext, RenderReader};
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{rt::track::PlayerResource, test_pools::pools};

    impl crate::RenderResource for Resource {
        fn spec(&self) -> AudioSpec {
            Resource::spec(self)
        }
        fn cached_span(&self) -> Duration {
            Resource::cached_span(self)
        }
        fn decoded_frontier(&self) -> Duration {
            Resource::decoded_frontier(self)
        }
        fn read_planar<'a>(
            &mut self,
            output: &'a mut [&'a mut [f32]],
        ) -> Result<ReadOutcome, DecodeError> {
            Resource::read_planar(self, output)
        }
        fn duration(&self) -> Option<Duration> {
            Resource::duration(self)
        }
        fn playback_rate(&self) -> f32 {
            unreachable!("slot test does not render")
        }
        fn apply_playback_rate(&self, _rate: f32) -> f32 {
            unreachable!("slot test does not render")
        }
        fn render_reader(&self) -> Option<RenderReader> {
            unreachable!("slot test does not render")
        }
        fn sync_seek(&mut self) {
            Resource::sync_seek(self);
        }
        fn clear_render(&self) {
            unreachable!("slot test does not render")
        }
        fn seek_handle(&self) -> Option<Arc<dyn SeekBegin>> {
            Resource::seek_handle(self)
        }
        fn set_host_sample_rate(&self, sample_rate: NonZeroU32) {
            Resource::set_host_sample_rate(self, sample_rate);
        }
        fn set_service_class(&self, _class: crate::worker::ServiceClass) {}
        fn publish_render(&self, _context: &RenderContext, _frontier: PresentationFrontier) {
            unreachable!("slot test does not render")
        }
    }

    fn track(item_id: TrackId, src: Arc<str>) -> PlayerTrack {
        let sample_rate = NonZeroU32::new(44_100).expect("static sample rate");
        let reader = Unimock::new((
            AudioSessionMock::event_bus
                .each_call(matching!())
                .answers(&|mock| mock.make_ref(EventBus::new(1))),
            AudioSessionMock::duration
                .each_call(matching!())
                .returns(Some(Duration::from_secs(1))),
            AudioReadMock::spec
                .each_call(matching!())
                .returns(AudioSpec::new(2, sample_rate)),
            AudioControlMock::preload
                .next_call(matching!())
                .returns(Ok(())),
        ));
        let resource = Resource::from_reader(reader, Some(Arc::clone(&src)));
        let resource = PlayerResource::new(resource, src, &pools())
            .map_or_else(|error| panic!("test player resource: {error}"), Box::new);

        PlayerTrack::builder()
            .sample_rate(sample_rate)
            .item_id(item_id)
            .build(resource)
    }

    #[kithara::test]
    fn identical_sources_are_addressed_by_track_id() {
        let src: Arc<str> = Arc::from("same.mp3");
        let first_id = TrackId::allocate();
        let second_id = TrackId::allocate();
        let mut tracks = TrackSlots::<2>::default();

        assert!(tracks.insert(track(first_id, Arc::clone(&src))).is_none());
        assert!(tracks.insert(track(second_id, src)).is_none());
        assert_eq!(
            tracks.get(first_id).map(PlayerTrack::item_id),
            Some(first_id)
        );
        assert_eq!(
            tracks.get(second_id).map(PlayerTrack::item_id),
            Some(second_id)
        );

        let removed = tracks
            .slot_of(first_id)
            .and_then(|slot| tracks.remove_at(slot))
            .map(|track| track.item_id());
        assert_eq!(removed, Some(first_id));
        assert!(tracks.get(first_id).is_none());
        assert_eq!(
            tracks.get(second_id).map(PlayerTrack::item_id),
            Some(second_id)
        );
    }
}
