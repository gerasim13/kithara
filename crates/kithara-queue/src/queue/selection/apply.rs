use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::Resource;
use tracing::{debug, warn};

use crate::{
    attempts::AttemptReport,
    event::QueueEvent,
    queue::{Queue, types::SelectPhase},
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Apply what a load attempt reported, and admit the resource its live
    /// attempt finished with.
    pub(in crate::queue) fn apply_report(&mut self, report: AttemptReport) {
        if let Some((id, resource)) = self.tracks.apply_report(report) {
            self.apply_loaded(id, resource);
        }
    }

    /// Admit a finished load and apply the selection that waited for it.
    /// The executor holding the queue runs it as one command, so no select
    /// interleaves between the finish and the selection.
    pub(in crate::queue) fn apply_loaded(&mut self, id: TrackId, resource: Resource) {
        if self.is_closed() {
            return;
        }

        let index = {
            let guard = self.tracks.lock();
            guard.iter().position(|entry| entry.id == id)
        };
        let Some(index) = index else {
            debug!(
                id = id.as_u64(),
                "load completed but track no longer in queue"
            );
            return;
        };

        self.tracks.admit(id, resource);
        if self
            .tracks
            .lock()
            .get(index)
            .is_some_and(|entry| entry.id == id)
        {
            self.bus.publish(QueueEvent::NextTrackReady { id, index });
        }

        self.apply_pending_selection(id);
    }

    fn apply_pending_selection(&mut self, id: TrackId) {
        let selection = match self.pending_select {
            SelectPhase::Pending(pending) if pending.id == id => {
                self.pending_select = SelectPhase::Idle;
                Some(pending)
            }
            _ => None,
        };

        if self.autoplay_target == Some(id) {
            self.autoplay_target = None;
        }
        let Some(selection) = selection else {
            return;
        };
        if let Err(error) =
            self.select_loaded_item(id, selection.settings, selection.reason, selection.playback)
        {
            warn!(id = id.as_u64(), error = %error, "pending select failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_audio::mock::TestPcmReader;
    use kithara_decode::TrackMetadata;
    use kithara_platform::{CancelToken, sync::Arc};
    use kithara_play::{ResourceConfig, ResourceSrc};
    use kithara_signal::AudioSpec;
    use kithara_test_utils::{cancel_token, kithara};

    use super::*;
    use crate::{
        event::TrackStatus,
        queue::state::tests::{make_queue, make_store},
    };

    /// Admission keeps the caller's metadata and a cover placed before it,
    /// and fills only the fields they leave unset from the decoder's tags.
    #[kithara::test(tokio, flash(false))]
    async fn admission_fills_unset_metadata_from_the_decoder(cancel_token: CancelToken) {
        let mut queue = make_queue();
        let url = "https://example.com/opaque.m3u8";
        let mut append = |title: Option<&str>| {
            let config =
                ResourceConfig::for_src(ResourceSrc::parse(url).expect("valid source URL"))
                    .store(make_store())
                    .metadata(TrackMetadata {
                        title: title.map(Into::into),
                        album: Some("Catalogue album".into()),
                        ..TrackMetadata::default()
                    })
                    .build();
            queue.append(config).expect("append configured track")
        };
        let titled = append(Some("Catalogue title"));
        let untitled = append(None);
        let track = |queue: &Queue<crate::test_pools::TestPools>, id| {
            queue.track(id).expect("the track stays queued")
        };
        assert_eq!(
            track(&queue, titled).metadata().title.as_deref(),
            Some("Catalogue title")
        );
        let cover = Arc::new(vec![1, 2, 3]);
        queue
            .tracks
            .place_cover(titled, &cancel_token, Arc::clone(&cover));

        let spec = AudioSpec::new(2, crate::consts::TEST_SAMPLE_RATE);
        for id in [titled, untitled] {
            let reader = TestPcmReader::new(spec, 0.01);
            queue.apply_loaded(id, Resource::from_reader(reader, Some(Arc::from(url))));
            assert!(matches!(
                track(&queue, id).status,
                TrackStatus::Loaded | TrackStatus::Consumed
            ));
        }

        let admitted = track(&queue, titled);
        let metadata = admitted.metadata();
        assert_eq!(metadata.title.as_deref(), Some("Catalogue title"));
        assert_eq!(metadata.album.as_deref(), Some("Catalogue album"));
        assert_eq!(metadata.artwork, Some(cover));
        assert_eq!(
            track(&queue, untitled).metadata().title.as_deref(),
            Some("Mock")
        );
        kithara_play::player::Player::close(&mut queue)
            .expect("close the queue before deferred loads run");
    }
}
