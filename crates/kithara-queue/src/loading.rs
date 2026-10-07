use kithara_bufpool::HasPool;
use kithara_command::Seq;
use kithara_events::TrackId;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_play::ResourceConfig;

use crate::error::QueueError;

/// Which loader lane a load occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadClass {
    /// User-facing selection: one dedicated lane slot, isolated from
    /// prefetch so a hung background lane cannot starve selection.
    Interactive,
    /// Append-time background prefetch, capped by
    /// [`QueueConfig::max_concurrent_loads`](crate::QueueConfig::max_concurrent_loads).
    Prefetch,
}

/// What a task beside a track's load reports to the queue that owns the
/// track.
pub(crate) enum LoadReport {
    /// The downloader found the load's transfer slow. `watch` ends with the
    /// load, so a report from a load that ended since is past news.
    Slow { id: TrackId, watch: CancelToken },
    /// The track's cover, read beside its audio. `load` is the track's token,
    /// which outlives the load in the resource it built: the audio never
    /// waits for the cover.
    Cover {
        id: TrackId,
        load: CancelToken,
        cover: Arc<Vec<u8>>,
    },
    /// The track's token ended the load: a load still waiting for its lane
    /// is over; one already sent ends with its open's answer.
    Cancelled { id: TrackId },
}

/// Why an open the dispatcher answered left its track without a source.
pub(crate) enum OpenFailure {
    /// A cause a later ask can answer.
    AskAgain(QueueError),
    /// A cause no later ask answers.
    Final(QueueError),
}

/// A track's live load: its prepared config, the lane it waits for, and the
/// open it has in flight. Dropping it armed cancels the track's token, so
/// removing a track aborts its load; dropping it always ends the tasks that
/// watch the load.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct TrackLoad<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) config: ResourceConfig<S>,
    /// The lane the load waits for until it is sent; it keeps the lane it
    /// was sent in.
    pub(crate) class: LoadClass,
    /// The user's selection wants this track. A prefetch already sent stays
    /// in its lane when the selection reaches it; being wanted decides
    /// whether a refusal a later ask can answer is asked again.
    pub(crate) selected: bool,
    /// The open in flight, `None` while the load waits for its lane.
    pub(crate) sent: Option<Seq>,
    /// The track's token; once the load opens it belongs to the resource.
    #[field(get, vis = "pub(crate)")]
    token: CancelToken,
    /// Ends the tasks watching the load, however the load ends.
    #[field(get, vis = "pub(crate)")]
    watch: CancelToken,
    /// Whether dropping the load cancels `token`.
    armed: bool,
}

impl<S> TrackLoad<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// A load of `config` waiting for `class`'s lane.
    ///
    /// # Errors
    /// [`QueueError::Resource`] when `config` carries no per-track token.
    pub(crate) fn new(config: ResourceConfig<S>, class: LoadClass) -> Result<Self, QueueError> {
        let Some(token) = config.cancel().cloned() else {
            return Err(QueueError::Resource(
                "resource config missing per-track cancel".to_owned(),
            ));
        };
        Ok(Self {
            watch: token.child(),
            token,
            config,
            class,
            selected: class == LoadClass::Interactive,
            sent: None,
            armed: true,
        })
    }

    /// Give the track's token up to its next owner; dropping then cancels
    /// nothing but the load's watches.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        !self.armed || self.token.is_cancelled()
    }
}

impl<S> Drop for TrackLoad<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
        self.watch.cancel();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use kithara_assets::AssetStore;
    use kithara_play::ResourceSrc;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::{TestPools, pools};

    /// A config for `url` carrying `token` as its per-track token.
    pub(crate) fn config(url: &str, token: CancelToken) -> ResourceConfig<TestPools> {
        let mut config =
            ResourceConfig::for_src(ResourceSrc::parse(url).expect("BUG: test URL is valid"))
                .store(AssetStore::builder(pools()).build())
                .build();
        config.set_cancel(token);
        config
    }

    fn load(token: &CancelToken) -> TrackLoad<TestPools> {
        TrackLoad::new(
            config("https://x/a.mp3", token.clone()),
            LoadClass::Prefetch,
        )
        .expect("the config carries its token")
    }

    #[kithara::test]
    fn dropping_an_armed_load_cancels_its_track() {
        let token = CancelToken::never().child();
        drop(load(&token));
        assert!(token.is_cancelled());
    }

    #[kithara::test]
    fn dropping_a_disarmed_load_leaves_its_track_to_the_resource() {
        let token = CancelToken::never().child();
        let mut load = load(&token);
        let watch = load.watch().clone();
        load.disarm();
        drop(load);
        assert!(!token.is_cancelled());
        assert!(watch.is_cancelled(), "a load's watches end with it");
    }

    #[kithara::test]
    fn a_config_without_a_track_token_is_refused() {
        let config = ResourceConfig::<TestPools>::for_src(
            ResourceSrc::parse("https://x/a.mp3").expect("BUG: test URL is valid"),
        )
        .store(AssetStore::builder(pools()).build())
        .build();
        assert!(matches!(
            TrackLoad::new(config, LoadClass::Prefetch),
            Err(QueueError::Resource(_))
        ));
    }
}
