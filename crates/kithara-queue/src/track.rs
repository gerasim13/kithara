use kithara_audio::{AudioObserver, AudioObserverRelay, AudioObserverSlot};
use kithara_bufpool::HasPool;
use kithara_decode::TrackMetadata;
use kithara_events::{EventBus, TrackId};
use kithara_platform::{
    CancelToken,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};
use kithara_play::{Resource, ResourceConfig, ResourceSrc};
use tracing::debug;

use crate::{
    attempts::{AttemptGuard, AttemptReport, Ticket},
    error::QueueError,
    event::{QueueEvent, TrackStatus},
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

/// Single owner of everything the queue knows about one track. Dropping the record aborts its
/// attempt via [`AttemptGuard`].
pub(crate) struct TrackRecord<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) load: Option<AttemptGuard>,
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
}

/// Authoritative store for the queue's track list.
///
/// Single owner of `Vec<TrackRecord>`. Only the [`Queue`](crate::Queue)
/// writes it; a load attempt's task reports its transitions to the queue,
/// which applies them through [`Tracks::apply_report`]. Every status
/// transition MUST go through [`Tracks::set_status`] (or the attempt ops
/// below) so the polled view and the reactive
/// [`QueueEvent::TrackStatusChanged`] stream never drift.
pub(crate) struct Tracks<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    next_generation: AtomicU64,
    bus: EventBus,
    inner: Mutex<Vec<TrackRecord<S>>>,
}

impl<S> Tracks<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) fn new(bus: EventBus) -> Self {
        Self {
            bus,
            inner: Mutex::new(Vec::new()),
            next_generation: AtomicU64::new(0),
        }
    }

    /// Hold a finished load for `id` until the deck takes it: fill the
    /// metadata the caller left unset from the decoder's tags and mark the
    /// track `Loaded`. No-op when `id` is not present.
    pub(crate) fn admit(&self, id: TrackId, resource: Resource) {
        let mut guard = self.lock();
        let Some(record) = guard.iter_mut().find(|record| record.id == id) else {
            return;
        };
        record.metadata.fill_missing_from(resource.metadata());
        record.resource = Some(resource);
        drop(guard);
        self.set_status(id, TrackStatus::Loaded);
    }

    /// Set the status a live, uncancelled attempt reports and publish it,
    /// running `transition` on the attempt first. A report from any other
    /// attempt changes nothing: the track has moved on from it.
    fn advance(&self, ticket: &Ticket, transition: impl FnOnce(&mut AttemptGuard) -> TrackStatus) {
        let mut guard = self.lock();
        let status = guard
            .iter_mut()
            .find(|record| record.id == ticket.id)
            .and_then(|record| {
                let attempt = record.load.as_mut().filter(|attempt| {
                    attempt.generation == ticket.generation && !attempt.is_cancelled()
                })?;
                record.status = transition(attempt);
                Some(record.status.clone())
            });
        drop(guard);
        if let Some(status) = status {
            self.bus.publish(QueueEvent::TrackStatusChanged {
                id: ticket.id,
                status,
            });
        }
    }

    /// Apply what a load attempt reported. Returns the resource a live
    /// attempt finished with, for the queue to admit.
    pub(crate) fn apply_report(&self, report: AttemptReport) -> Option<(TrackId, Resource)> {
        match report {
            AttemptReport::Started(ticket) => {
                self.advance(&ticket, |attempt| {
                    attempt.waiting = false;
                    TrackStatus::Loading
                });
                None
            }
            AttemptReport::Slow(ticket) => {
                self.advance(&ticket, |_| TrackStatus::Slow);
                None
            }
            AttemptReport::Cover { id, attempt, cover } => {
                self.place_cover(id, &attempt, cover);
                None
            }
            AttemptReport::Finished { ticket, outcome } => self
                .finish_attempt(&ticket, outcome)
                .map(|resource| (ticket.id, resource)),
        }
    }

    /// Attach decoded-audio observation to this track's current resource, or
    /// retain it for resource admission when loading has not started yet.
    pub(crate) fn attach_observer(&self, id: TrackId, observer: Box<dyn AudioObserver>) {
        let slot = self
            .lock()
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.observer.clone());
        let Some(slot) = slot else {
            return;
        };
        slot.attach(observer);
    }

    /// Whether the user's selection wants this track's live attempt.
    ///
    /// Read by the attempt itself, so it reflects a selection that arrived
    /// after the attempt started.
    pub(crate) fn attempt_selected(&self, id: TrackId) -> bool {
        let guard = self.lock();
        let selected = guard
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.load.as_ref())
            .is_some_and(|a| a.selected);
        drop(guard);
        selected
    }

    /// Register a fresh attempt. Dedupes against a live attempt; replaces
    /// one that is already cancelled but still unwinding.
    pub(crate) fn begin_attempt(
        &self,
        id: TrackId,
        cancel: CancelToken,
        selected: bool,
    ) -> Option<Ticket> {
        let mut guard = self.lock();
        let ticket = match guard.iter_mut().find(|r| r.id == id) {
            Some(record) if record.load.as_ref().is_none_or(AttemptGuard::is_cancelled) => {
                Some(install(record, &self.next_generation, cancel, selected))
            }
            _ => None,
        };
        drop(guard);
        ticket
    }

    /// Attempt finished. While the ticket is its track's live attempt, the
    /// guard is disarmed and removed (the token now belongs to the built
    /// `Resource`, or died with the dropped load future), a failure flips the
    /// track to `Failed`, and a resource is handed back for admission. A stale
    /// ticket's outcome is dropped: the track moved on, and dropping its guard
    /// cancelled that resource's token.
    fn finish_attempt(
        &self,
        ticket: &Ticket,
        outcome: Result<Box<Resource>, QueueError>,
    ) -> Option<Resource> {
        let attempt = self
            .lock()
            .iter_mut()
            .find(|record| record.id == ticket.id)
            .and_then(|record| {
                record
                    .load
                    .take_if(|attempt| attempt.generation == ticket.generation)
            });
        let Some(mut attempt) = attempt else {
            debug!(
                id = ticket.id.as_u64(),
                "a superseded load attempt ended; dropping its outcome"
            );
            return None;
        };
        attempt.disarm();
        match outcome {
            Ok(resource) => Some(*resource),
            Err(QueueError::Cancelled(_)) => None,
            Err(error) => {
                self.set_status(ticket.id, TrackStatus::Failed(error.to_string()));
                None
            }
        }
    }

    /// Place the cover a load attempt read for `id` and publish
    /// [`QueueEvent::TrackMetadataChanged`], unless that attempt's token
    /// `attempt` is cancelled: a superseded attempt's cover is dropped.
    pub(crate) fn place_cover(&self, id: TrackId, attempt: &CancelToken, cover: Arc<Vec<u8>>) {
        let mut guard = self.lock();
        let Some(record) = guard
            .iter_mut()
            .find(|record| record.id == id)
            .filter(|_| !attempt.is_cancelled())
        else {
            return;
        };
        record.metadata.artwork = Some(cover);
        drop(guard);
        self.bus.publish(QueueEvent::TrackMetadataChanged { id });
    }

    /// Lock the underlying `Vec<TrackRecord>` for direct read/write.
    /// Callers that only need to flip status should prefer
    /// [`Self::set_status`].
    pub(crate) fn lock(&self) -> MutexGuard<'_, Vec<TrackRecord<S>>> {
        self.inner.lock()
    }

    /// Create the decoder half before resource opening and install its
    /// control half in canonical per-track state. Any observer attached before
    /// admission is transferred into the same bounded relay.
    pub(crate) fn observer_relay(&self, id: TrackId) -> AudioObserverRelay {
        let slot = self
            .lock()
            .iter()
            .find(|record| record.id == id)
            .map(|record| record.observer.clone())
            .unwrap_or_default();
        slot.relay()
    }

    /// Move a track's pending load into the interactive lane: replace a
    /// still-waiting (or cancelled-but-unwinding) attempt. An attempt
    /// already holding a permit is kept - its download is progressing.
    /// Vacant means the attempt just finished; the completion path owns
    /// what happens next, so no new attempt starts.
    pub(crate) fn promote_attempt(&self, id: TrackId, cancel: CancelToken) -> Option<Ticket> {
        let mut guard = self.lock();
        let ticket = match guard.iter_mut().find(|r| r.id == id) {
            Some(record)
                if record
                    .load
                    .as_ref()
                    .is_some_and(|a| a.waiting || a.is_cancelled()) =>
            {
                Some(install(record, &self.next_generation, cancel, true))
            }
            Some(record) => {
                if let Some(attempt) = record.load.as_mut() {
                    attempt.selected = true;
                }
                None
            }
            None => None,
        };
        drop(guard);
        ticket
    }

    /// Atomically mutate `record.status` and publish
    /// [`QueueEvent::TrackStatusChanged`]. `Cancelled` and `Loaded` also
    /// abort the track's live attempt: a cancelled track never keeps
    /// loading, and a track whose resource is already loaded has nothing
    /// left to load. Without the latter an attempt that outlives the
    /// resource it was meant to fetch reports its own outcome afterwards
    /// and overwrites a track that is already playable. Every status but
    /// `Loaded` drops the resource the record holds: only a loaded track
    /// has audio waiting for the deck.
    /// No-op when `id` is not present (caller raced `Queue::remove`).
    pub(crate) fn set_status(&self, id: TrackId, status: TrackStatus) {
        let mut guard = self.lock();
        let Some(record) = guard.iter_mut().find(|r| r.id == id) else {
            return;
        };
        record.status = status.clone();
        let aborted = matches!(status, TrackStatus::Cancelled | TrackStatus::Loaded)
            .then(|| record.load.take())
            .flatten();
        let dropped = (!matches!(status, TrackStatus::Loaded))
            .then(|| record.resource.take())
            .flatten();
        drop(guard);
        drop(aborted);
        drop(dropped);
        self.bus
            .publish(QueueEvent::TrackStatusChanged { id, status });
    }

    /// Hand `id`'s loaded resource to the caller, which gives it to the deck.
    /// `None` once the deck holds it, or before the track loads.
    pub(crate) fn take_resource(&self, id: TrackId) -> Option<Resource> {
        self.lock()
            .iter_mut()
            .find(|record| record.id == id)
            .and_then(|record| record.resource.take())
    }

    /// Original source for `id`, if still queued.
    pub(crate) fn source(&self, id: TrackId) -> Option<TrackSource<S>> {
        self.lock()
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.source.clone())
    }
}

fn install<S>(
    record: &mut TrackRecord<S>,
    generations: &AtomicU64,
    cancel: CancelToken,
    selected: bool,
) -> Ticket
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    let generation = generations.fetch_add(1, Ordering::Relaxed);
    let mut attempt = AttemptGuard::new(generation, cancel);
    attempt.selected = selected;
    record.load = Some(attempt);
    Ticket {
        generation,
        id: record.id,
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::AssetStore;
    use kithara_audio::{AudioObserveError, AudioObserver};
    use kithara_platform::sync::{Arc, atomic::AtomicUsize};
    use kithara_signal::{AudioChunk, AudioChunkInfo};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::test_pools::{TestPools, pools, sample_buffer};

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
        tracks.lock()[0].status.clone()
    }

    /// `ticket`'s attempt ended on a cancel.
    fn cancelled(ticket: Ticket) -> AttemptReport {
        AttemptReport::Finished {
            ticket,
            outcome: Err(QueueError::Cancelled(ticket.id)),
        }
    }

    /// `ticket`'s attempt ended on `reason`.
    fn failed(ticket: Ticket, reason: &str) -> AttemptReport {
        AttemptReport::Finished {
            ticket,
            outcome: Err(QueueError::Resource(reason.to_owned())),
        }
    }

    fn tracks_with(id: TrackId) -> Tracks<TestPools> {
        let tracks = Tracks::new(EventBus::default());
        tracks.lock().push(TrackRecord::new(
            id,
            String::new(),
            "https://x/a.mp3".into(),
        ));
        tracks
    }

    /// Two queued tracks, each carrying its own source, so a lookup that
    /// reaches the wrong record is visible rather than indistinguishable.
    fn two_tracks() -> Tracks<TestPools> {
        let tracks = Tracks::new(EventBus::default());
        let mut guard = tracks.lock();
        guard.push(TrackRecord::new(
            TrackId(1),
            String::new(),
            "https://x/first.mp3".into(),
        ));
        guard.push(TrackRecord::new(
            TrackId(2),
            String::new(),
            "https://x/second.mp3".into(),
        ));
        drop(guard);
        tracks
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
        let mut relay = tracks.observer_relay(TrackId(2));

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
        let mut relay = tracks.observer_relay(TrackId(1));

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

    fn token() -> CancelToken {
        CancelToken::never().child()
    }

    #[kithara::test]
    fn begin_dedupes_live_attempt() {
        let tracks = tracks_with(TrackId(1));
        assert!(tracks.begin_attempt(TrackId(1), token(), false).is_some());
        assert!(tracks.begin_attempt(TrackId(1), token(), false).is_none());
    }

    #[kithara::test]
    fn selection_reflects_only_the_requested_live_attempt() {
        let tracks = two_tracks();
        assert!(!tracks.attempt_selected(TrackId(1)));
        assert!(tracks.begin_attempt(TrackId(1), token(), false).is_some());
        assert!(tracks.begin_attempt(TrackId(2), token(), true).is_some());
        assert!(!tracks.attempt_selected(TrackId(1)));
        assert!(tracks.attempt_selected(TrackId(2)));
    }

    #[kithara::test]
    fn begin_replaces_cancelled_unwinding_attempt() {
        let tracks = tracks_with(TrackId(1));
        let first_cancel = token();
        let first = tracks
            .begin_attempt(TrackId(1), first_cancel.clone(), false)
            .expect("BUG: vacant record must accept an attempt");
        tracks.set_status(TrackId(1), TrackStatus::Cancelled);
        assert!(first_cancel.is_cancelled(), "Cancelled must abort the load");
        let second = tracks
            .begin_attempt(TrackId(1), token(), false)
            .expect("cancelled attempt must be replaceable");
        tracks.apply_report(AttemptReport::Started(first));
        assert_eq!(
            status(&tracks),
            TrackStatus::Cancelled,
            "replaced ticket loses claim"
        );
        tracks.apply_report(AttemptReport::Started(second));
        assert_eq!(status(&tracks), TrackStatus::Loading);
    }

    /// A track whose resource is already in the player has nothing left
    /// to load. The attempt still in flight for it is fetching something
    /// nobody waits for, and which side of that race the machine picks
    /// must not decide whether the track is playable.
    #[kithara::test]
    fn a_loaded_track_is_not_failed_by_the_attempt_it_outlived() {
        let tracks = tracks_with(TrackId(1));
        let attempt = tracks
            .begin_attempt(TrackId(1), token(), false)
            .expect("BUG: vacant record must accept an attempt");

        tracks.set_status(TrackId(1), TrackStatus::Loaded);
        tracks.apply_report(failed(attempt, "HTTP 404"));

        assert!(matches!(tracks.lock()[0].status, TrackStatus::Loaded));
    }

    #[kithara::test]
    fn promote_replaces_waiting_and_cancels_it() {
        let tracks = tracks_with(TrackId(1));
        let parked_cancel = token();
        let parked = tracks
            .begin_attempt(TrackId(1), parked_cancel.clone(), false)
            .expect("BUG: vacant record must accept an attempt");
        let promoted = tracks
            .promote_attempt(TrackId(1), token())
            .expect("waiting attempt must be promotable");
        assert!(parked_cancel.is_cancelled(), "parked attempt must abort");
        tracks.apply_report(AttemptReport::Started(parked));
        assert_eq!(status(&tracks), TrackStatus::Pending);
        tracks.apply_report(AttemptReport::Started(promoted));
        assert_eq!(status(&tracks), TrackStatus::Loading);
    }

    #[kithara::test]
    fn promote_keeps_attempt_holding_permit() {
        let tracks = tracks_with(TrackId(1));
        let loading = tracks
            .begin_attempt(TrackId(1), token(), false)
            .expect("BUG: vacant record must accept an attempt");
        tracks.apply_report(AttemptReport::Started(loading));
        assert!(
            tracks.promote_attempt(TrackId(1), token()).is_none(),
            "an attempt past the permit gate keeps its download"
        );
    }

    #[kithara::test]
    fn promote_vacant_is_noop() {
        let tracks = tracks_with(TrackId(1));
        assert!(tracks.promote_attempt(TrackId(1), token()).is_none());
    }

    #[kithara::test]
    fn finish_disarms_and_ignores_stale_ticket() {
        let tracks = tracks_with(TrackId(1));
        let first_cancel = token();
        let old = tracks
            .begin_attempt(TrackId(1), first_cancel, false)
            .expect("BUG: vacant record must accept an attempt");
        let new = tracks
            .promote_attempt(TrackId(1), token())
            .expect("waiting attempt must be promotable");
        tracks.apply_report(cancelled(old));
        tracks.apply_report(AttemptReport::Started(new));
        assert_eq!(
            status(&tracks),
            TrackStatus::Loading,
            "stale finish must not evict"
        );
        tracks.apply_report(cancelled(new));
        assert!(
            tracks.begin_attempt(TrackId(1), token(), false).is_some(),
            "finished attempt must leave the record vacant"
        );
    }

    #[kithara::test]
    fn removing_record_cancels_attempt() {
        let tracks = tracks_with(TrackId(1));
        let cancel = token();
        let _ticket = tracks
            .begin_attempt(TrackId(1), cancel.clone(), false)
            .expect("BUG: vacant record must accept an attempt");
        tracks.lock().clear();
        assert!(cancel.is_cancelled(), "dropping the record aborts the load");
    }

    #[kithara::test]
    fn finish_with_failure_sets_failed_once() {
        let tracks = tracks_with(TrackId(1));
        let ticket = tracks
            .begin_attempt(TrackId(1), token(), false)
            .expect("BUG: vacant record must accept an attempt");
        tracks.apply_report(failed(ticket, "boom"));
        assert!(matches!(status(&tracks), TrackStatus::Failed(_)));
    }
}
