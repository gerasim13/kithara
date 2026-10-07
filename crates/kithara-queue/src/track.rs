use std::vec;

use kithara_audio::{AudioObserver, AudioObserverSlot};
use kithara_bufpool::HasPool;
use kithara_command::Seq;
use kithara_decode::TrackMetadata;
use kithara_events::TrackId;
use kithara_platform::{CancelToken, sync::Arc};
use kithara_play::{Resource, ResourceConfig, ResourceLoad, ResourceSrc};
use tracing::debug;

use crate::{
    error::QueueError,
    event::{QueueEvent, TrackStatus},
    loading::{LoadClass, LoadReport, OpenFailure, TrackLoad},
};

/// Snapshot of a track entry in the queue.
#[derive(Debug, Clone, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct TrackEntry {
    /// Canonical source location: a normalized URL or a file path.
    /// `None` only for a non-UTF-8 file path.
    pub url: Option<String>,
    /// Display name derived from the URL or caller-supplied. May be empty.
    pub name: String,
    /// The source's metadata, with its unset fields filled from the decoder's
    /// tags once the track's resource is admitted; the cover read from the
    /// source's artwork becomes its artwork.
    #[field(get)]
    metadata: TrackMetadata,
    /// Stable identifier.
    pub id: TrackId,
    /// Current loading status.
    pub status: TrackStatus,
}

/// Input to [`Queue::append`](crate::Queue::append) /
/// [`Queue::insert`](crate::Queue::insert) describing how to load a track.
///
/// Two shapes:
/// - [`TrackSource::Uri`] — the queue builds a default
///   [`ResourceConfig`] from the [`QueueConfig`](crate::QueueConfig) templates
///   (`net`, `store`). Convenient for simple use.
/// - [`TrackSource::Config`] — the caller pre-builds a [`ResourceConfig`]
///   (useful for DRM keys, custom headers, format hints). The queue leaves
///   caller-set fields intact.
///
/// `TrackSource` is `Clone` so the queue can respawn a load when a
/// previously-consumed track is re-selected — re-tapping a track in
/// the playlist must work without the caller reconstructing anything.
#[derive(derive_more::From)]
#[non_exhaustive]
#[derive_where::derive_where(Clone; S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static)]
pub enum TrackSource<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Load from URL / path. Queue fills in defaults from `QueueConfig`.
    #[from]
    Uri(String),
    /// Caller-assembled resource config (DRM, headers, etc.). Boxed because
    /// [`ResourceConfig`] is ~100 bytes larger than the `Uri` variant.
    #[from]
    Config(Box<ResourceConfig<S>>),
}

impl<S> TrackSource<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Canonical source location: the string for [`TrackSource::Uri`], the
    /// config's URL or file path for [`TrackSource::Config`]. `None` only
    /// for a non-UTF-8 file path.
    #[must_use]
    pub fn uri(&self) -> Option<&str> {
        match self {
            Self::Uri(s) => Some(s),
            Self::Config(cfg) => match cfg.source() {
                ResourceSrc::Url(url) => Some(url.as_str()),
                ResourceSrc::Path(path) => path.to_str(),
            },
        }
    }

    pub(crate) fn metadata(&self) -> Option<&TrackMetadata> {
        match self {
            Self::Config(config) => config.metadata(),
            Self::Uri(_) => None,
        }
    }
}

impl<S> From<&str> for TrackSource<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn from(s: &str) -> Self {
        Self::Uri(s.to_string())
    }
}

impl<S> From<ResourceConfig<S>> for TrackSource<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn from(c: ResourceConfig<S>) -> Self {
        Self::Config(Box::new(c))
    }
}

/// Single owner of everything the queue knows about one track. Dropping the
/// record aborts its load via [`TrackLoad`].
pub(crate) struct TrackRecord<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) load: Option<TrackLoad<S>>,
    pub(crate) url: Option<String>,
    pub(crate) name: String,
    /// Taken from the source at append; admission fills its unset fields.
    pub(crate) metadata: TrackMetadata,
    pub(crate) id: TrackId,
    pub(crate) source: TrackSource<S>,
    pub(crate) status: TrackStatus,
    observer: AudioObserverSlot,
    /// The finished load, held while the track is `Loaded` until the deck
    /// takes it.
    resource: Option<Resource>,
}

impl<S> TrackRecord<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) fn new(id: TrackId, name: String, source: TrackSource<S>) -> Self {
        Self {
            id,
            name,
            metadata: source.metadata().cloned().unwrap_or_default(),
            url: source.uri().map(str::to_string),
            status: TrackStatus::Pending,
            source,
            load: None,
            observer: AudioObserverSlot::default(),
            resource: None,
        }
    }

    pub(crate) fn entry(&self) -> TrackEntry {
        TrackEntry {
            id: self.id,
            name: self.name.clone(),
            metadata: self.metadata.clone(),
            url: self.url.clone(),
            status: self.status.clone(),
        }
    }

    /// The track's load while it still runs: present and not cancelled.
    fn live_load(&mut self) -> Option<&mut TrackLoad<S>> {
        self.load.as_mut().filter(|load| !load.is_cancelled())
    }
}

/// One track as the queue's handles read it: what [`TrackEntry`] shows, the
/// source to rebuild it from, and the slot that reaches its decoder.
pub(crate) struct TrackRow<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) entry: TrackEntry,
    pub(crate) source: TrackSource<S>,
    pub(crate) observer: AudioObserverSlot,
}

/// Authoritative store for the queue's track list.
///
/// Single owner of `Vec<TrackRecord>`, held by the [`Queue`](crate::Queue)
/// alone; what becomes of a track's load reaches it through the loader the
/// queue owns. Every status transition MUST go through
/// [`Tracks::set_status`] (or the load ops below), which records its
/// [`QueueEvent::TrackStatusChanged`] for the queue to announce, so the
/// published rows and the event stream never drift. Every edit moves
/// [`Tracks::revision`], which tells the queue the rows it published are out
/// of date.
#[derive_where::derive_where(Default)]
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Tracks<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Moves with every edit; equal revisions mean equal rows.
    #[field(get, vis = "pub(crate)", copy)]
    revision: u64,
    /// The changes made since the queue last took them, in order.
    events: Vec<QueueEvent>,
    /// The records, in queue order.
    #[field(get, vis = "pub(crate)")]
    records: Vec<TrackRecord<S>>,
}

impl<S> Tracks<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Hold a finished load for `id` until the deck takes it: fill the
    /// metadata the caller left unset from the decoder's tags and mark the
    /// track `Loaded`. No-op when `id` is not present.
    pub(crate) fn admit(&mut self, id: TrackId, resource: Resource) {
        let Some(record) = self.record_mut(id) else {
            return;
        };
        record.metadata.fill_missing_from(resource.metadata());
        record.resource = Some(resource);
        self.set_status(id, TrackStatus::Loaded);
    }

    /// Apply what a task beside a track's load reported.
    pub(crate) fn apply_report(&mut self, report: LoadReport) {
        match report {
            LoadReport::Slow { id, watch } => {
                if !watch.is_cancelled() {
                    self.set_status(id, TrackStatus::Slow);
                }
            }
            LoadReport::Cover { id, load, cover } => self.place_cover(id, &load, cover),
        }
    }

    /// Attach decoded-audio observation to this track's current resource, or
    /// retain it for resource admission when loading has not started yet.
    pub(crate) fn attach_observer(&self, id: TrackId, observer: Box<dyn AudioObserver>) {
        if let Some(record) = self.find(id) {
            record.observer.attach(observer);
        }
    }

    /// Give `id` a fresh load and return it. Dedupes against a live load,
    /// which keeps running; replaces one that is already cancelled.
    pub(crate) fn begin_load(&mut self, id: TrackId, load: TrackLoad<S>) -> Option<&TrackLoad<S>> {
        self.revision += 1;
        let record = self.records.iter_mut().find(|record| {
            record.id == id && record.load.as_ref().is_none_or(TrackLoad::is_cancelled)
        })?;
        record.load = Some(load);
        record.load.as_ref()
    }

    /// Fail `id` with `error`. Its load gives the track's token up rather
    /// than cancelling it: a cover read beside the load still lands.
    pub(crate) fn fail(&mut self, id: TrackId, error: &QueueError) {
        if let Some(mut load) = self.record_mut(id).and_then(|record| record.load.take()) {
            load.disarm();
        }
        self.set_status(id, TrackStatus::Failed(error.to_string()));
    }

    fn find(&self, id: TrackId) -> Option<&TrackRecord<S>> {
        self.records.iter().find(|record| record.id == id)
    }

    /// The selection reached `id`: its live load is wanted, and one still
    /// waiting moves to the interactive lane; one already sent keeps its
    /// lane, its download progressing. `false` when `id` has no live load.
    pub(crate) fn promote_load(&mut self, id: TrackId) -> bool {
        self.revision += 1;
        let Some(load) = self.record_mut(id).and_then(TrackRecord::live_load) else {
            return false;
        };
        load.selected = true;
        if load.sent.is_none() {
            load.class = LoadClass::Interactive;
        }
        true
    }

    /// The first live load in queue order waiting for `class`'s lane, as the
    /// dispatcher opens it: its config, with the track's observer slot
    /// reaching the decoder it builds.
    pub(crate) fn next_unsent(&self, class: LoadClass) -> Option<(TrackId, ResourceLoad<S>)> {
        self.records.iter().find_map(|record| {
            let load = record.load.as_ref().filter(|load| {
                !load.is_cancelled() && load.sent.is_none() && load.class == class
            })?;
            let observer = Box::new(record.observer.relay());
            Some((record.id, ResourceLoad::new(load.config.clone(), observer)))
        })
    }

    /// `id`'s load went to the dispatcher as `seq`: the track loads.
    pub(crate) fn mark_sent(&mut self, id: TrackId, seq: Seq) {
        let Some(load) = self.record_mut(id).and_then(TrackRecord::live_load) else {
            return;
        };
        load.sent = Some(seq);
        let loading = self.find(id).is_some_and(|record| {
            matches!(record.status, TrackStatus::Loading | TrackStatus::Slow)
        });
        if !loading {
            self.set_status(id, TrackStatus::Loading);
        }
    }

    /// Settle the open sent as `seq`. Returns the resource it opened, for
    /// the queue to admit.
    ///
    /// An open no live record sent belongs to a load the track moved on from,
    /// and is dropped. A cancel is the last word on a load, whatever its open
    /// ended with. A refusal a later ask can answer leaves a selected load
    /// waiting for its lane again, so a track chosen during an outage plays
    /// once the network answers; a load nobody selected fails, so nothing
    /// waits for it.
    pub(crate) fn settle_load(
        &mut self,
        seq: Seq,
        opened: Result<Resource, OpenFailure>,
    ) -> Option<(TrackId, Resource)> {
        let Some(record) = self.records.iter_mut().find(|record| {
            record
                .load
                .as_ref()
                .is_some_and(|load| load.sent == Some(seq))
        }) else {
            debug!(
                seq = seq.get(),
                "a superseded load's open ended; dropping it"
            );
            return None;
        };
        let id = record.id;
        if record.load.as_ref().is_some_and(TrackLoad::is_cancelled) {
            self.set_status(id, TrackStatus::Cancelled);
            return None;
        }
        match opened {
            Ok(resource) => {
                if let Some(mut load) = record.load.take() {
                    load.disarm();
                }
                Some((id, resource))
            }
            Err(OpenFailure::AskAgain(error)) => {
                match record.load.as_mut().filter(|load| load.selected) {
                    Some(load) => {
                        debug!(?id, %error, "a selected load failed on a cause a later ask can answer; asking again");
                        load.sent = None;
                    }
                    None => self.fail(id, &error),
                }
                None
            }
            Err(OpenFailure::Final(error)) => {
                self.fail(id, &error);
                None
            }
        }
    }

    /// Place the cover a load read for `id` and record
    /// [`QueueEvent::TrackMetadataChanged`], unless that load's token `load`
    /// is cancelled: a superseded load's cover is dropped.
    pub(crate) fn place_cover(&mut self, id: TrackId, load: &CancelToken, cover: Arc<Vec<u8>>) {
        if load.is_cancelled() {
            return;
        }
        let Some(record) = self.record_mut(id) else {
            return;
        };
        record.metadata.artwork = Some(cover);
        self.events.push(QueueEvent::TrackMetadataChanged { id });
    }

    fn record_mut(&mut self, id: TrackId) -> Option<&mut TrackRecord<S>> {
        self.records_mut().iter_mut().find(|record| record.id == id)
    }

    /// The records for a direct edit. Callers that only need to flip status
    /// should prefer [`Self::set_status`].
    pub(crate) fn records_mut(&mut self) -> &mut Vec<TrackRecord<S>> {
        self.revision += 1;
        &mut self.records
    }

    /// Take the changes recorded since the last take, in order, for the
    /// queue to announce.
    pub(crate) fn drain_events(&mut self) -> vec::Drain<'_, QueueEvent> {
        self.events.drain(..)
    }

    /// The rows the queue publishes for its handles, in queue order.
    pub(crate) fn rows(&self) -> Arc<[TrackRow<S>]> {
        self.records
            .iter()
            .map(|record| TrackRow {
                entry: record.entry(),
                source: record.source.clone(),
                observer: record.observer.clone(),
            })
            .collect()
    }

    /// Set `record.status` and record [`QueueEvent::TrackStatusChanged`].
    /// `Cancelled` and `Loaded` also abort the track's live load: a
    /// cancelled track never keeps loading, and a track whose resource is
    /// already loaded has nothing left to load. Without the latter a load
    /// that outlives the resource it was meant to fetch settles afterwards
    /// and overwrites a track that is already playable. Every status but
    /// `Loaded` drops the resource the record holds: only a loaded track has
    /// audio waiting for the deck.
    /// No-op when `id` is not present (caller raced `Queue::remove`).
    pub(crate) fn set_status(&mut self, id: TrackId, status: TrackStatus) {
        let Some(record) = self.record_mut(id) else {
            return;
        };
        record.status = status.clone();
        let aborted = matches!(status, TrackStatus::Cancelled | TrackStatus::Loaded)
            .then(|| record.load.take())
            .flatten();
        let dropped = (!matches!(status, TrackStatus::Loaded))
            .then(|| record.resource.take())
            .flatten();
        drop(aborted);
        drop(dropped);
        self.events
            .push(QueueEvent::TrackStatusChanged { id, status });
    }

    /// Cancel every live load: the queue closed, and nothing it loads is
    /// wanted any more.
    pub(crate) fn cancel_loads(&mut self) {
        let loading: Vec<TrackId> = self
            .records
            .iter()
            .filter(|record| record.load.is_some())
            .map(|record| record.id)
            .collect();
        for id in loading {
            self.set_status(id, TrackStatus::Cancelled);
        }
    }

    delegate::delegate! {
        to self {
            /// Hand `id`'s loaded resource to the caller, which gives it to the deck.
            /// `None` once the deck holds it, or before the track loads.
            #[expr($.and_then(|record| record.resource.take()))]
            #[call(record_mut)]
            pub(crate) fn take_resource(&mut self, id: TrackId) -> Option<Resource>;
            /// Original source for `id`, if still queued.
            #[expr($.map(|record| record.source.clone()))]
            #[call(find)]
            pub(crate) fn source(&self, id: TrackId) -> Option<TrackSource<S>>;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use kithara_assets::AssetStore;
    use kithara_audio::{AudioObserveError, AudioObserver, mock::TestPcmReader};
    use kithara_command::{Batch, ChannelConfig, When, channel};
    use kithara_platform::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use kithara_play::DispatcherProtocol;
    use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        consts::TEST_SAMPLE_RATE,
        loading::tests::config,
        test_pools::{TestPools, pools, sample_buffer},
    };

    #[kithara::test]
    #[case::from_str("https://example.com/song.mp3")]
    #[case::from_string("https://example.com/track.m3u8")]
    fn track_source_from_string_kind(#[case] url: &str) {
        let owned = url.to_string();
        let from_owned: TrackSource<TestPools> = owned.into();
        assert_eq!(from_owned.uri(), Some(url));
        let from_ref: TrackSource<TestPools> = url.into();
        assert_eq!(from_ref.uri(), Some(url));
    }

    #[kithara::test]
    fn track_source_from_resource_config() {
        let src =
            ResourceSrc::parse("https://example.com/a.mp3").expect("BUG: hard-coded URL is valid");
        let cfg = ResourceConfig::for_src(src)
            .store(AssetStore::builder(pools()).build())
            .build();
        let src: TrackSource<TestPools> = cfg.into();
        assert!(matches!(src, TrackSource::Config(_)));
        assert_eq!(src.uri(), Some("https://example.com/a.mp3"));
    }

    /// The first track's status.
    fn status(tracks: &Tracks<TestPools>) -> TrackStatus {
        tracks.records()[0].status.clone()
    }

    /// Numbers a real channel gives its sends, as the loader's sends get them.
    fn seqs() -> impl Iterator<Item = Seq> {
        let (mut sender, inbox) = channel::<DispatcherProtocol<ResourceLoad<TestPools>>>(
            ChannelConfig::builder().build(),
        );
        iter::from_fn(move || {
            let _executor = &inbox;
            sender
                .send(
                    When::Next,
                    Batch {
                        basis: Vec::new(),
                        commands: Vec::new(),
                    },
                )
                .ok()
        })
    }

    fn next_seq(seqs: &mut impl Iterator<Item = Seq>) -> Seq {
        seqs.next().expect("the channel has room")
    }

    fn refusal(reason: &str) -> QueueError {
        QueueError::Resource(reason.to_owned())
    }

    /// Begin a `class` load for `id` and return the track's token.
    fn begin(tracks: &mut Tracks<TestPools>, id: TrackId, class: LoadClass) -> CancelToken {
        let token = CancelToken::never().child();
        let load = TrackLoad::new(config("https://x/a.mp3", token.clone()), class)
            .expect("the config carries its token");
        assert!(
            tracks.begin_load(id, load).is_some(),
            "BUG: the record must take the load"
        );
        token
    }

    /// Begin a `class` load for the first track and send it as `seq`.
    fn sent(tracks: &mut Tracks<TestPools>, class: LoadClass, seq: Seq) -> CancelToken {
        let id = tracks.records()[0].id;
        let token = begin(tracks, id, class);
        let (unsent, _load) = tracks
            .next_unsent(class)
            .expect("the load waits for its lane");
        assert_eq!(unsent, id);
        tracks.mark_sent(id, seq);
        token
    }

    fn opened() -> Resource {
        let reader = TestPcmReader::new(AudioSpec::new(2, TEST_SAMPLE_RATE), 0.01);
        Resource::from_reader(reader, None)
    }

    fn tracks_with(id: TrackId) -> Tracks<TestPools> {
        let mut tracks = Tracks::default();
        tracks.records_mut().push(TrackRecord::new(
            id,
            String::new(),
            "https://x/a.mp3".into(),
        ));
        tracks
    }

    /// Two queued tracks, each carrying its own source, so a lookup that
    /// reaches the wrong record is visible rather than indistinguishable.
    fn two_tracks() -> Tracks<TestPools> {
        let mut tracks = Tracks::default();
        let records = tracks.records_mut();
        records.push(TrackRecord::new(
            TrackId(1),
            String::new(),
            "https://x/first.mp3".into(),
        ));
        records.push(TrackRecord::new(
            TrackId(2),
            String::new(),
            "https://x/second.mp3".into(),
        ));
        tracks
    }

    /// The slot a load of `id` hands its decoder.
    fn slot(tracks: &Tracks<TestPools>, id: TrackId) -> &AudioObserverSlot {
        &tracks.find(id).expect("the track is queued").observer
    }

    struct CountingObserver(Arc<AtomicUsize>);

    impl AudioObserver for CountingObserver {
        fn try_observe(&mut self, _chunk: &AudioChunk) -> Result<(), AudioObserveError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[kithara::test]
    fn an_attached_observer_reaches_its_own_tracks_decoder() {
        let pools = pools();
        let tracks = two_tracks();
        let seen = Arc::new(AtomicUsize::new(0));
        tracks.attach_observer(TrackId(2), Box::new(CountingObserver(Arc::clone(&seen))));
        let mut relay = slot(&tracks, TrackId(2)).relay();

        let chunk = AudioChunk::new(AudioChunkInfo::default(), sample_buffer(&pools, &[]));
        relay.try_observe(&chunk).expect("the observer accepts it");

        assert_eq!(seen.load(Ordering::Relaxed), 1);
    }

    #[kithara::test]
    fn an_attached_observer_does_not_reach_another_track() {
        let pools = pools();
        let tracks = two_tracks();
        let seen = Arc::new(AtomicUsize::new(0));
        tracks.attach_observer(TrackId(2), Box::new(CountingObserver(Arc::clone(&seen))));
        let mut relay = slot(&tracks, TrackId(1)).relay();

        let chunk = AudioChunk::new(AudioChunkInfo::default(), sample_buffer(&pools, &[]));
        relay
            .try_observe(&chunk)
            .expect("an empty relay is a no-op");

        assert_eq!(seen.load(Ordering::Relaxed), 0);
    }

    #[kithara::test]
    fn a_track_reports_its_own_source() {
        let tracks = two_tracks();

        assert_eq!(
            tracks
                .source(TrackId(2))
                .as_ref()
                .and_then(TrackSource::uri),
            Some("https://x/second.mp3")
        );
    }

    #[kithara::test]
    fn an_unqueued_track_has_no_source() {
        let tracks = two_tracks();

        assert!(tracks.source(TrackId(3)).is_none());
    }

    /// Whether the selection wants `id`'s live load.
    fn selected(tracks: &Tracks<TestPools>, id: TrackId) -> bool {
        tracks
            .records()
            .iter()
            .any(|record| record.id == id && record.load.as_ref().is_some_and(|load| load.selected))
    }

    /// Nobody selected the load the network refused for now, so nobody waits
    /// for it: its track fails with the refusal, and the load is not sent
    /// again.
    #[kithara::test]
    fn an_unselected_load_refused_for_now_fails_its_track() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        sent(&mut tracks, LoadClass::Prefetch, seq);

        assert!(
            tracks
                .settle_load(
                    seq,
                    Err(OpenFailure::AskAgain(refusal("connection refused")))
                )
                .is_none()
        );

        assert_eq!(
            status(&tracks),
            TrackStatus::Failed(refusal("connection refused").to_string())
        );
        assert!(
            tracks.next_unsent(LoadClass::Prefetch).is_none(),
            "an abandoned load was sent again"
        );
    }

    /// The selection reached the track after its prefetch was sent. The
    /// queue answers the refusal against the selection as it stands, so the
    /// track keeps loading: the load waits for its own lane again.
    #[kithara::test]
    fn a_load_selected_after_it_was_sent_is_asked_again() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        let token = sent(&mut tracks, LoadClass::Prefetch, seq);
        assert!(tracks.promote_load(TrackId(1)));

        tracks.settle_load(
            seq,
            Err(OpenFailure::AskAgain(refusal("connection refused"))),
        );

        assert_eq!(status(&tracks), TrackStatus::Loading);
        assert!(!token.is_cancelled(), "the wanted load was cut off");
        assert!(
            matches!(
                tracks.next_unsent(LoadClass::Prefetch),
                Some((TrackId(1), _))
            ),
            "the wanted load is not asked again in its lane"
        );
    }

    #[kithara::test]
    fn begin_dedupes_a_live_load() {
        let mut tracks = tracks_with(TrackId(1));
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
        let token = CancelToken::never().child();
        let second = TrackLoad::new(config("https://x/a.mp3", token), LoadClass::Prefetch)
            .expect("the config carries its token");
        assert!(tracks.begin_load(TrackId(1), second).is_none());
    }

    #[kithara::test]
    fn selection_reflects_only_the_requested_live_load() {
        let mut tracks = two_tracks();
        assert!(!selected(&tracks, TrackId(1)));
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
        begin(&mut tracks, TrackId(2), LoadClass::Interactive);
        assert!(!selected(&tracks, TrackId(1)));
        assert!(selected(&tracks, TrackId(2)));
    }

    /// A load whose token was cancelled is over, though its open has not
    /// answered yet: a fresh load replaces it, and the old open's answer
    /// lands on nothing.
    #[kithara::test]
    fn begin_replaces_a_cancelled_load() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let first = next_seq(&mut seqs);
        sent(&mut tracks, LoadClass::Prefetch, first).cancel();

        let second = next_seq(&mut seqs);
        sent(&mut tracks, LoadClass::Prefetch, second);
        assert!(tracks.settle_load(first, Ok(opened())).is_none());
        assert_eq!(
            status(&tracks),
            TrackStatus::Loading,
            "the replaced load settled"
        );
        assert!(tracks.settle_load(second, Ok(opened())).is_some());
    }

    /// A track whose resource is already in the player has nothing left
    /// to load. The open still in flight for it fetches something nobody
    /// waits for, and which side of that race the machine picks must not
    /// decide whether the track is playable.
    #[kithara::test]
    fn a_loaded_track_is_not_failed_by_the_load_it_outlived() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        sent(&mut tracks, LoadClass::Prefetch, seq);

        tracks.set_status(TrackId(1), TrackStatus::Loaded);
        tracks.settle_load(seq, Err(OpenFailure::Final(refusal("HTTP 404"))));

        assert!(matches!(tracks.records()[0].status, TrackStatus::Loaded));
    }

    /// A prefetch still waiting for its lane is what the selection wants
    /// next: it moves to the interactive lane instead of waiting behind
    /// background loads.
    #[kithara::test]
    fn promote_moves_a_waiting_load_to_the_interactive_lane() {
        let mut tracks = tracks_with(TrackId(1));
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);

        assert!(tracks.promote_load(TrackId(1)));

        assert!(tracks.next_unsent(LoadClass::Prefetch).is_none());
        assert!(matches!(
            tracks.next_unsent(LoadClass::Interactive),
            Some((TrackId(1), _))
        ));
    }

    #[kithara::test]
    fn promote_keeps_a_sent_load_in_its_lane() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let token = sent(&mut tracks, LoadClass::Prefetch, next_seq(&mut seqs));

        assert!(tracks.promote_load(TrackId(1)));

        assert!(
            tracks.next_unsent(LoadClass::Interactive).is_none(),
            "a load already sent keeps its download"
        );
        assert!(!token.is_cancelled());
    }

    #[kithara::test]
    fn promote_without_a_live_load_reports_none() {
        let mut tracks = tracks_with(TrackId(1));
        assert!(!tracks.promote_load(TrackId(1)));
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch).cancel();
        assert!(!tracks.promote_load(TrackId(1)));
    }

    /// A load that opened hands its token to the resource and leaves the
    /// record free for the next load.
    #[kithara::test]
    fn an_opened_load_disarms_and_leaves_its_record_vacant() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        let token = sent(&mut tracks, LoadClass::Prefetch, seq);

        assert!(matches!(
            tracks.settle_load(seq, Ok(opened())),
            Some((TrackId(1), _))
        ));

        assert!(!token.is_cancelled(), "the resource's token was cancelled");
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
    }

    #[kithara::test]
    fn removing_a_record_cancels_its_load() {
        let mut tracks = tracks_with(TrackId(1));
        let token = begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
        tracks.records_mut().clear();
        assert!(token.is_cancelled(), "dropping the record aborts the load");
    }

    /// The caller's cancel reached the load's token while its open's answer
    /// was on its way to the queue. The load is over, and its track says so
    /// instead of loading forever.
    #[kithara::test]
    fn a_load_ended_by_its_cancel_leaves_its_track_cancelled() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        let token = sent(&mut tracks, LoadClass::Prefetch, seq);

        token.cancel();
        tracks.settle_load(seq, Err(OpenFailure::Final(refusal("cancelled"))));

        assert_eq!(status(&tracks), TrackStatus::Cancelled);
    }

    /// A resource the load opened before the caller cancelled it, and that
    /// the queue settles only afterwards, is not admitted: the cancel is the
    /// last word on that load.
    #[kithara::test]
    fn a_resource_whose_load_was_cancelled_is_not_admitted() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        let token = sent(&mut tracks, LoadClass::Prefetch, seq);

        token.cancel();

        assert!(
            tracks.settle_load(seq, Ok(opened())).is_none(),
            "a cancelled load's resource reached admission"
        );
        assert_eq!(status(&tracks), TrackStatus::Cancelled);
    }

    #[kithara::test]
    fn a_failed_load_fails_its_track() {
        let mut seqs = seqs();
        let mut tracks = tracks_with(TrackId(1));
        let seq = next_seq(&mut seqs);
        sent(&mut tracks, LoadClass::Prefetch, seq);
        tracks.settle_load(seq, Err(OpenFailure::Final(refusal("boom"))));
        assert_eq!(
            status(&tracks),
            TrackStatus::Failed(refusal("boom").to_string())
        );
    }

    /// A slow transfer is news only while its load runs: a report from a
    /// load that ended since changes nothing.
    #[kithara::test]
    fn only_a_running_load_turns_its_track_slow() {
        let mut tracks = tracks_with(TrackId(1));
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
        let watch = |tracks: &Tracks<TestPools>| {
            tracks.records()[0]
                .load
                .as_ref()
                .map(|load| load.watch().clone())
                .expect("the track loads")
        };
        let ended = watch(&tracks);
        tracks.set_status(TrackId(1), TrackStatus::Cancelled);
        begin(&mut tracks, TrackId(1), LoadClass::Prefetch);

        tracks.apply_report(LoadReport::Slow {
            id: TrackId(1),
            watch: ended,
        });
        assert_eq!(status(&tracks), TrackStatus::Cancelled);

        let running = watch(&tracks);
        tracks.apply_report(LoadReport::Slow {
            id: TrackId(1),
            watch: running,
        });
        assert_eq!(status(&tracks), TrackStatus::Slow);
    }

    #[kithara::test]
    fn cancelling_loads_cancels_every_live_load() {
        let mut tracks = two_tracks();
        let first = begin(&mut tracks, TrackId(1), LoadClass::Prefetch);
        let second = begin(&mut tracks, TrackId(2), LoadClass::Interactive);

        tracks.cancel_loads();

        assert!(first.is_cancelled() && second.is_cancelled());
        assert!(
            tracks
                .records()
                .iter()
                .all(|record| record.status == TrackStatus::Cancelled)
        );
    }
}
