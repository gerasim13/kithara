use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_platform::tokio::task;
use kithara_play::Resource;
use tracing::{debug, warn};

use crate::{
    error::QueueError,
    event::{QueueEvent, TrackStatus},
    queue::{QueueControl, types::SelectPhase},
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Apply a finished load synchronously, off the runtime.
    ///
    /// Takes the admission lock and dispatches through the session's
    /// synchronous command bridge, so the caller waits for a reply. On a
    /// runtime worker that wait parks the executor thread.
    ///
    /// The lock is held across the whole synchronous block, never across an `.await`, so the
    /// `Cancelled` re-check and the item selection stay atomic with respect to a concurrent apply.
    fn apply_loaded(&self, id: TrackId, resource: Resource) {
        let _admission = self.lock_admission();
        if self.is_closed() {
            return;
        }

        let _apply = self.lock_select_apply();

        if self.player.is_closed() {
            return;
        }

        let was_cancelled = self
            .tracks
            .lock()
            .iter()
            .find(|entry| entry.id == id)
            .is_some_and(|entry| matches!(entry.status, TrackStatus::Cancelled));
        if was_cancelled {
            debug!(
                id = id.as_u64(),
                "load was overridden by a later select; dropping its resource"
            );
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

    fn apply_pending_selection(&self, id: TrackId) {
        let selection = {
            let mut phase = self.pending_select.lock();
            let selection = match *phase {
                SelectPhase::Pending(pending) if pending.id == id => {
                    *phase = SelectPhase::Idle;
                    Some(pending)
                }
                _ => None,
            };
            drop(phase);
            selection
        };

        self.autoplay_target.disarm_if_matches(id);
        let Some(selection) = selection else {
            return;
        };
        if let Err(error) =
            self.select_loaded_item(id, selection.settings, selection.reason, selection.playback)
        {
            warn!(id = id.as_u64(), error = %error, "pending select failed");
        }
    }

    pub(super) fn watch_apply(
        &self,
        id: TrackId,
        handle: Option<task::JoinHandle<Result<Resource, QueueError>>>,
    ) {
        if self.is_closed() {
            return;
        }
        let Some(handle) = handle else {
            return;
        };
        let queue = self.clone();
        drop(self.loader.spawn(async move {
            let resource = match handle.await {
                Ok(Ok(resource)) => resource,
                Ok(Err(_)) => return,
                Err(join_err) => {
                    warn!(id = id.as_u64(), error = %join_err, "loader join failed");
                    return;
                }
            };
            drop(task::spawn_sync(move || {
                queue.apply_loaded(id, resource);
            }));
        }));
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
    use crate::queue::state::tests::{make_queue, make_store};

    /// Admission keeps the caller's metadata and a cover placed before it,
    /// and fills only the fields they leave unset from the decoder's tags.
    #[kithara::test(tokio, flash(false))]
    async fn admission_fills_unset_metadata_from_the_decoder(cancel_token: CancelToken) {
        let queue = make_queue();
        let url = "https://example.com/opaque.m3u8";
        let append = |title: Option<&str>| {
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
        let track = |id| queue.track(id).expect("the track stays queued");
        assert_eq!(
            track(titled).metadata().title.as_deref(),
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
                track(id).status,
                TrackStatus::Loaded | TrackStatus::Consumed
            ));
        }

        let admitted = track(titled);
        let metadata = admitted.metadata();
        assert_eq!(metadata.title.as_deref(), Some("Catalogue title"));
        assert_eq!(metadata.album.as_deref(), Some("Catalogue album"));
        assert_eq!(metadata.artwork, Some(cover));
        assert_eq!(track(untitled).metadata().title.as_deref(), Some("Mock"));
        queue
            .close()
            .expect("close the queue before deferred loads run");
    }
}
