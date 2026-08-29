use std::{
    collections::{HashMap, VecDeque},
    io::{Error as IoError, ErrorKind},
};

use kithara::{
    analysis::{AnalysisFingerprint, AnalysisToken, TrackAnalysis},
    assets::{
        AcquisitionResult, AssetResource, AssetResourceState, AssetStore, AssetsError, ReadSide,
        ResourceKey, WriteSide,
    },
    bufpool::BytePool,
    decode::DecodeError,
    prelude::ResourceConfig,
};
use tracing::{debug, warn};

/// Tunables for the analysis cache, grouped to keep the module surface small.
struct Consts;

impl Consts {
    /// Cap on the in-memory tier; past it the oldest entries fall back to disk.
    const MAX_MEM_ENTRIES: usize = 64;
}

/// Physical analysis resource together with the store that owns it.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct AnalysisTarget {
    store: AssetStore,
    #[field(get, vis = "pub(crate)")]
    key: ResourceKey,
}

impl AnalysisTarget {
    pub(crate) fn for_config(config: &ResourceConfig) -> Result<Self, DecodeError> {
        let key = config.asset_key(&AssetResource::Named {
            namespace: "analysis".to_string(),
            name: "track.analysis".to_string(),
        })?;
        Ok(Self {
            key,
            store: config.store().clone(),
        })
    }

    pub(crate) fn is_same(&self, other: &Self) -> bool {
        self.key == other.key && self.store.is_same(&other.store)
    }
}

struct MemoryEntry {
    target: AnalysisTarget,
    analysis: TrackAnalysis,
}

/// Two-tier track-analysis memoization: a session in-memory map plus durable
/// blobs stored as resources of each track's `AssetScope` (so they follow the
/// track's storage lifecycle). Owned by the single listener task, so it needs
/// no synchronization.
pub(crate) struct TrackAnalysisCache {
    byte_pool: BytePool,
    mem: HashMap<ResourceKey, Vec<MemoryEntry>>,
    /// Active analysis configuration, per artifact: a stored artifact whose
    /// tag differs is dropped on its own, so a waveform resolution change no
    /// longer invalidates stored beat results.
    fingerprint: AnalysisFingerprint,
    /// Insertion order of store-qualified targets; the oldest is evicted past
    /// the cap.
    order: VecDeque<AnalysisTarget>,
}

impl TrackAnalysisCache {
    pub(crate) fn new(fingerprint: AnalysisFingerprint, byte_pool: BytePool) -> Self {
        Self {
            byte_pool,
            fingerprint,
            mem: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Whether a cached snapshot carries every artifact the active
    /// configuration expects. A stored artifact whose tag moved is dropped on
    /// read, so a hit can be real and still need the pass to run.
    pub(crate) fn is_sufficient(&self, analysis: &TrackAnalysis) -> bool {
        let waveform = self.fingerprint.waveform().is_none() || analysis.waveform().is_some();
        let beat = self.fingerprint.beat().is_none() || analysis.beat().is_some();
        waveform && beat
    }

    /// Look up a cached analysis: memory first, then the scope resource.
    /// `None` on a miss or an unreadable blob.
    pub(crate) fn get(&mut self, target: &AnalysisTarget) -> Option<TrackAnalysis> {
        if let Some(analysis) = self.mem.get(&target.key).and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.target.is_same(target))
                .map(|entry| entry.analysis.clone())
        }) {
            return Some(analysis);
        }
        let analysis = self.load_disk(target)?;
        self.remember(target.clone(), analysis.clone());
        Some(analysis)
    }

    fn load_disk(&self, target: &AnalysisTarget) -> Option<TrackAnalysis> {
        let resource = &target.key;
        // Side-effect-free probe first: opening a missing key would create it.
        match target.store.resource_state(resource).ok()? {
            AssetResourceState::Committed { .. } => {}
            _ => return None,
        }
        let reader = target.store.open_resource(resource, None).ok()?;
        let mut bytes = self.byte_pool.get();
        reader.read_into(&mut bytes).ok()?;
        match TrackAnalysis::try_from((&bytes[..], &self.fingerprint)) {
            Ok(analysis) => {
                debug!("track analysis cache: disk hit");
                Some(analysis)
            }
            Err(e) => {
                warn!(%e, ?resource, "track analysis cache: ignoring stale/unreadable blob");
                None
            }
        }
    }

    /// Store freshly derived track analysis in both tiers.
    pub(crate) fn put(&mut self, target: AnalysisTarget, analysis: TrackAnalysis) {
        // An analysis with no meaningful slots would be served forever as
        // emptiness on later hits; skip memoizing it in either tier.
        if analysis.waveform().is_none() && analysis.beat().is_none() {
            return;
        }
        // A pass publishes while it decodes, so a run cut short leaves a
        // partial snapshot behind. Caching it would serve that partial result
        // as the whole track on every later hit. A pass that ran out of
        // reachable ranges is not that: what it holds is what the content
        // gives, gaps the source refuses included.
        if !analysis.is_settled() {
            debug!(
                completeness = ?analysis.waveform_completeness(),
                "track analysis cache: unsettled snapshot left uncached"
            );
            return;
        }
        self.store_disk(&target, &analysis);
        self.remember(target, analysis);
    }

    /// Insert into the bounded memory tier, evicting the oldest entry past
    /// [`Consts::MAX_MEM_ENTRIES`]. Evicted entries are still served from disk.
    fn remember(&mut self, target: AnalysisTarget, analysis: TrackAnalysis) {
        let entries = self.mem.entry(target.key.clone()).or_default();
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.target.is_same(&target))
        {
            entry.analysis = analysis;
            return;
        }

        entries.push(MemoryEntry {
            analysis,
            target: target.clone(),
        });
        self.order.push_back(target);

        while self.order.len() > Consts::MAX_MEM_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                let bucket_is_empty = self.mem.get_mut(old.key()).is_some_and(|entries| {
                    entries.retain(|entry| !entry.target.is_same(&old));
                    entries.is_empty()
                });
                if bucket_is_empty {
                    self.mem.remove(old.key());
                }
            }
        }
    }

    fn store_disk(&self, target: &AnalysisTarget, analysis: &TrackAnalysis) {
        let resource = &target.key;
        let mut bytes = self.byte_pool.get();
        if let Err(e) = analysis.write_to(&mut bytes) {
            warn!(%e, ?resource, "track analysis cache: encode failed");
            return;
        }
        if let Err(e) = write_resource(&target.store, resource, &bytes) {
            warn!(%e, ?resource, "track analysis cache: blob write failed");
        }
    }
}

fn write_resource(
    store: &AssetStore,
    resource: &ResourceKey,
    bytes: &[u8],
) -> Result<(), AssetsError> {
    let writer = match store.acquire_resource(resource, None)? {
        AcquisitionResult::Pending(writer) => writer,
        AcquisitionResult::Ready(reader) => reader.reactivate()?,
        _ => return Ok(()),
    };
    writer.write_at(0, bytes)?;
    let final_len = u64::try_from(bytes.len()).map_err(|_| {
        AssetsError::Io(IoError::new(
            ErrorKind::InvalidInput,
            "track analysis blob length does not fit u64",
        ))
    })?;
    writer.commit(Some(final_len))?;
    Ok(())
}

/// The token a stored blob carries: derived from the resource key the blob
/// lives under, so a restored snapshot identifies the same content it was
/// analysed from rather than a session-scoped id.
pub(crate) fn token_for(key: &ResourceKey) -> AnalysisToken {
    match (key.asset_root(), key.rel_path()) {
        (Some(root), Some(rel)) => format!("{root}/{rel}").into(),
        _ => key
            .as_absolute_path()
            .map_or_else(|| "unkeyed".into(), |path| path.display().to_string())
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, path::Path};

    // The test macro import shadows the `kithara` crate name; use absolute path.
    use ::kithara::{
        analysis::{
            AnalysisFingerprint, BeatGrid, BeatSnapshot, Coverage, FrameRange, GridState,
            TrackAnalysis, Waveform,
        },
        assets::{
            AssetLayout, AssetLayoutRegistry, AssetResource, AssetResourceState, AssetSource,
            AssetStore, StorageBackend,
        },
        bufpool::BytePool,
        file::File,
        prelude::ResourceConfig,
        warp::GridSegment,
    };
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;
    use url::Url;

    use super::{AnalysisTarget, Consts, TrackAnalysisCache};

    /// The beat tag two tests must agree on: one of them keeps it while the
    /// waveform tag moves.
    const BEAT_TAG: &str = "beat:test:v1";

    fn fingerprint(wave: &str, beat: &str) -> AnalysisFingerprint {
        AnalysisFingerprint::new(Some(beat), Some(wave))
    }

    fn fp() -> AnalysisFingerprint {
        fingerprint("wave:native:max1500:v1", BEAT_TAG)
    }

    fn rate() -> NonZeroU32 {
        NonZeroU32::new(44_100).expect("fixture rate is non-zero")
    }

    fn wave() -> Waveform {
        // version 1 + one bucket of three 0.5 band heights (0.5 = 0x3F000000).
        Waveform::try_from([1, 0, 0, 0, 0, 0, 0, 63, 0, 0, 0, 63, 0, 0, 0, 63].as_slice())
            .expect("hand-built blob is valid")
    }

    fn grid() -> BeatGrid {
        BeatGrid::new(
            128.0,
            vec![(0, Some(0.9)), (10_000, Some(0.75)), (20_000, None)],
            vec![(0, Some(0.9)), (40_000, None)],
            vec![GridSegment::new(0, 40_000, 1.01)],
        )
    }

    fn analysis(beat: Option<BeatGrid>, waveform: Option<Waveform>, extent: u64) -> TrackAnalysis {
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, extent));
        TrackAnalysis::builder()
            .token("assets/track.analysis".into())
            .revision(7)
            .source_sample_rate(rate())
            .extent(extent)
            .settled(true)
            .coverage(coverage)
            .fingerprint(fp())
            .maybe_waveform(waveform)
            .maybe_beat(beat.map(|grid| {
                BeatSnapshot::new(grid, GridState::Provisional, vec![FrameRange::new(100, 50)])
            }))
            .build()
    }

    fn full_analysis() -> TrackAnalysis {
        analysis(Some(grid()), Some(wave()), 1_234_567)
    }

    fn store_in(dir: &Path) -> AssetStore {
        AssetStore::builder()
            .backend(StorageBackend::Disk { root: dir.into() })
            .build()
    }

    fn memory_store() -> AssetStore {
        AssetStore::builder()
            .backend(StorageBackend::Memory)
            .build()
    }

    fn config(store: &AssetStore, src: &str, discriminator: Option<&str>) -> ResourceConfig {
        let builder =
            ResourceConfig::for_src(ResourceConfig::parse_src(src).expect("valid test source"))
                .store(store.clone());
        match discriminator {
            Some(discriminator) => builder.discriminator(discriminator).build(),
            None => builder.build(),
        }
    }

    fn target_for(store: &AssetStore, src: &str, discriminator: Option<&str>) -> AnalysisTarget {
        AnalysisTarget::for_config(&config(store, src, discriminator))
            .expect("test source has a layout-owned analysis target")
    }

    fn target(store: &AssetStore, discriminator: &str) -> AnalysisTarget {
        target_for(
            store,
            "https://analysis.test.invalid/track.mp3",
            Some(discriminator),
        )
    }

    fn analysis_cache() -> TrackAnalysisCache {
        TrackAnalysisCache::new(fp(), BytePool::default())
    }

    #[kithara::test]
    fn source_identity_ignores_query_without_a_discriminator() {
        let store = memory_store();
        let a = target_for(&store, "https://h.example/track/streamhq.mp3?id=123", None);
        let b = target_for(&store, "https://h.example/track/streamhq.mp3?id=456", None);
        assert_eq!(
            a.key(),
            b.key(),
            "query credentials do not fragment one logical asset"
        );

        let again = target_for(&store, "https://h.example/track/streamhq.mp3?id=123", None);
        assert_eq!(a.key(), again.key(), "keys are stable across calls");
    }

    #[kithara::test]
    fn explicit_discriminator_separates_query_selected_content() {
        let store = memory_store();
        let src = "https://h.example/track/streamhq.mp3?id=123";
        let a = target_for(&store, src, Some("content-a"));
        let b = target_for(&store, src, Some("content-b"));

        assert_ne!(a.key(), b.key());
    }

    #[kithara::test]
    fn config_target_is_stable_and_layout_owned() {
        let store = memory_store();
        let cfg = config(&store, "https://h.example/a.mp3?token=1", None);
        let first = AnalysisTarget::for_config(&cfg).expect("config source is keyable");
        let second = AnalysisTarget::for_config(&cfg).expect("config source is keyable");

        assert_eq!(first.key(), second.key());
        assert_eq!(first.key().rel_path(), Some("analysis/track.analysis"));
    }

    #[kithara::test(native)]
    fn local_path_sources_are_keyable() {
        let store = memory_store();
        let target = AnalysisTarget::for_config(&config(&store, "/tmp/song.mp3", None));
        assert!(target.is_ok(), "local files must cache their analysis");
    }

    #[derive(Debug)]
    struct InvalidLayout;

    impl AssetLayout for InvalidLayout {
        fn path(&self, _resource: &AssetResource) -> String {
            "../escape".to_string()
        }

        fn root(&self, _source: &AssetSource) -> String {
            "root".to_string()
        }
    }

    #[kithara::test]
    fn invalid_layout_is_not_treated_as_an_uncacheable_source() {
        let layouts = AssetLayoutRegistry::default().with::<File>(Arc::new(InvalidLayout));
        let store = AssetStore::builder()
            .backend(StorageBackend::Memory)
            .layouts(layouts)
            .build();
        let target =
            AnalysisTarget::for_config(&config(&store, "https://h.example/track.mp3", None));

        assert!(
            target.is_err(),
            "invalid layout output must remain an error"
        );
    }

    #[kithara::test]
    fn memory_store_round_trips() {
        let store = memory_store();
        let target = target(&store, "root_a");
        let mut cache = analysis_cache();
        assert!(cache.get(&target).is_none());
        cache.put(target.clone(), full_analysis());
        let cached = cache.get(&target).expect("analysis must be cached");
        assert_eq!(cached.waveform().expect("waveform cached").len(), 1);
        assert!(cached.beat().is_some(), "beat grid rides along");
    }

    #[kithara::test]
    fn same_key_in_different_stores_keeps_distinct_memory_entries() {
        let first_store = memory_store();
        let second_store = memory_store();
        let src = "https://analysis.test.invalid/shared.mp3";
        let first = target_for(&first_store, src, None);
        let second = target_for(&second_store, src, None);
        assert_eq!(first.key(), second.key());
        assert!(!first.is_same(&second));

        let mut cache = analysis_cache();
        cache.put(first.clone(), analysis(None, Some(wave()), 111));
        cache.put(second.clone(), analysis(None, Some(wave()), 222));

        assert_eq!(
            cache
                .get(&first)
                .expect("first store entry")
                .source_frames(),
            111
        );
        assert_eq!(
            cache
                .get(&second)
                .expect("second store entry")
                .source_frames(),
            222
        );
        assert_eq!(cache.order.len(), 2);
        assert_eq!(cache.mem.get(first.key()).map(Vec::len), Some(2));
    }

    #[kithara::test]
    fn empty_analysis_is_not_memoized() {
        let store = memory_store();
        let target = target(&store, "root_empty");
        let mut cache = analysis_cache();
        cache.put(target.clone(), analysis(None, None, 0));
        assert!(
            cache.get(&target).is_none(),
            "an analysis with no slots must not be served from the cache"
        );
    }

    #[kithara::test]
    fn memory_tier_is_bounded_with_disk_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let mut cache = analysis_cache();
        let oldest = target(&store, "root_0");
        for i in 0..=Consts::MAX_MEM_ENTRIES {
            cache.put(target(&store, &format!("root_{i}")), full_analysis());
        }
        assert!(
            cache.order.len() <= Consts::MAX_MEM_ENTRIES,
            "memory tier stays bounded under a whole-library sweep"
        );
        assert!(
            !cache.mem.contains_key(oldest.key()),
            "oldest entry evicted"
        );
        assert!(
            cache.get(&oldest).is_some(),
            "evicted entry is still served from the disk tier"
        );
    }

    #[kithara::test]
    fn disk_survives_a_fresh_cache_instance() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let target = target(&store, "root_b");
        let mut writer = analysis_cache();
        writer.put(target.clone(), full_analysis());

        // A new cache with an empty memory tier must still find the blob.
        let mut reader = analysis_cache();
        let cached = reader.get(&target).expect("disk analysis must load");
        assert_eq!(cached.waveform().expect("waveform persisted").len(), 1);
        assert_eq!(cached.beat().expect("beat grid persisted").grid(), &grid());
    }

    #[kithara::test]
    fn artifact_is_a_scope_resource_and_dies_with_the_asset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let url = Url::parse("https://analysis.test.invalid/track.mp3").expect("valid test URL");
        let discriminator = "track_root";
        let target = target_for(&store, url.as_str(), Some(discriminator));
        let mut cache = analysis_cache();
        cache.put(target.clone(), full_analysis());

        assert!(
            matches!(
                store.resource_state(target.key()),
                Ok(AssetResourceState::Committed { .. })
            ),
            "analysis blob must be a committed resource under the track scope"
        );
        // Deleting the asset takes the analysis with it.
        let source = AssetSource::Remote {
            url,
            discriminator: Some(discriminator.to_string()),
        };
        let scope = store.scope::<File>(&source).expect("valid analysis scope");
        scope.delete_asset().expect("asset deletes");
        let mut fresh = analysis_cache();
        assert!(
            fresh.get(&target).is_none(),
            "analysis must follow the track asset's lifecycle"
        );
    }

    /// A stored blob carries the tags of whatever produced it, so a snapshot
    /// from another configuration is what makes the read a miss.
    fn stored_under(fingerprint: AnalysisFingerprint) -> TrackAnalysis {
        let stored = full_analysis();
        TrackAnalysis::builder()
            .token(stored.token().clone())
            .revision(stored.revision())
            .source_sample_rate(stored.source_sample_rate())
            .maybe_extent(stored.extent())
            .settled(stored.is_settled())
            .coverage(stored.coverage().clone())
            .fingerprint(fingerprint)
            .maybe_waveform(stored.waveform().cloned())
            .maybe_beat(stored.beat().cloned())
            .build()
    }

    #[kithara::test]
    fn stale_fingerprint_blob_is_re_analysed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let target = target(&store, "root_fp");
        let mut old = analysis_cache();
        old.put(
            target.clone(),
            stored_under(fingerprint("other-wave", "other-beat")),
        );

        let mut current = analysis_cache();
        assert!(
            current.get(&target).is_none(),
            "a blob from another analysis config must be a miss"
        );
        // Overwriting with the current config works.
        current.put(target.clone(), full_analysis());
        let mut fresh = analysis_cache();
        assert!(fresh.get(&target).is_some());
    }

    #[kithara::test]
    fn a_partial_snapshot_is_not_cached() {
        let store = memory_store();
        let target = target(&store, "root_partial");
        let mut cache = analysis_cache();

        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(0, 500));
        let partial = TrackAnalysis::builder()
            .token("assets/track.analysis".into())
            .revision(3)
            .source_sample_rate(rate())
            .extent(1_000)
            .coverage(coverage)
            .fingerprint(fp())
            .waveform(wave())
            .build();
        assert_eq!(partial.waveform_completeness(), Some(0.5));

        cache.put(target.clone(), partial);
        assert!(
            cache.get(&target).is_none(),
            "half a track must not be memoized as the whole of it"
        );
    }

    #[kithara::test]
    fn a_settled_snapshot_is_cached_even_with_a_gap_left_in_it() {
        let store = memory_store();
        let target = target(&store, "root_settled");
        let mut cache = analysis_cache();

        // Encoder priming: the source cannot deliver its first frames, so the
        // pass ended with them uncovered and nothing left to try.
        let mut coverage = Coverage::default();
        coverage.insert(FrameRange::new(20, 980));
        let settled = TrackAnalysis::builder()
            .token("assets/track.analysis".into())
            .revision(3)
            .source_sample_rate(rate())
            .extent(1_000)
            .settled(true)
            .coverage(coverage)
            .fingerprint(fp())
            .waveform(wave())
            .build();
        assert!(!settled.is_complete(), "a gap is left at the head");

        cache.put(target.clone(), settled);
        assert!(
            cache.get(&target).is_some(),
            "a pass with nothing left to reach must not be re-run every launch"
        );
    }

    #[kithara::test]
    fn a_bucket_count_change_keeps_the_stored_beat_grid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        let target = target(&store, "root_split");
        let mut stored = analysis_cache();
        stored.put(target.clone(), full_analysis());

        // Only the waveform resolution moved: its own tag changes, the beat
        // backend's does not.
        let mut current = TrackAnalysisCache::new(
            fingerprint("wave:native:max3000:v1", BEAT_TAG),
            BytePool::default(),
        );
        let hit = current
            .get(&target)
            .expect("a beat-only hit must survive a waveform resolution change");
        assert_eq!(
            hit.beat().expect("the grid is still usable").grid(),
            &grid()
        );
        assert!(
            hit.waveform().is_none(),
            "the waveform was analysed at another resolution and must be dropped"
        );
    }
}
