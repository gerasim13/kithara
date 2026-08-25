use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use kithara::{
    assets::{
        AssetResource, AssetResourceState, AssetSource, AssetStore, AssetsError, ReadSide,
        ResourceKey, StorageBackend,
    },
    audio::{
        AssetAxis, AssetFrame, BeatEvidence, BeatGrid, BeatMapId, BeatMapSnapshot, BeatMarker,
        BeatOrdinal, FrameUncertainty, MapAxis, MapSegment, MapState, Meter, MeterFacts,
        SegmentFacts, SegmentSet, Waveform,
        analysis::{BeatAnalysisConfig, TrackAnalysis},
    },
    hls::AbrMode,
    platform::CancelToken,
    play::{PlaybackResamplerBackend, ResourceConfig},
};
use kithara_app::waveform::TrackAnalysisRunner;
use num_traits::ToPrimitive;
use sha2::{Digest, Sha256};

use super::{
    SyncFixtureError, SyncFixtureResources, SyncFixtureResult, blocking, hex_digest,
    library::{AnalysisProfile, SyncTrackFixture},
};
use crate::{
    assets_ext::write_new_resource,
    fixture_cache::{FixtureCache, fixture_cache_dir},
};

const ANALYSIS_BUCKETS: usize = 96_000;
const ANALYSIS_NAMESPACE: &str = "analysis";
const ANALYSIS_LOCK_DOMAIN: &str = "sync-track-analysis-lock-v1";
const ANALYSIS_KEY_MAGIC: &[u8] = b"kithara-sync-analysis-key\0";
const ANALYSIS_MAGIC: &[u8; 4] = b"KSAN";
const ANALYSIS_VERSION: u32 = 1;
const CACHE_CHECKSUM_BYTES: usize = 32;
const MAX_ANALYSIS_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CachedTrackAnalysis {
    analysis: TrackAnalysis,
    key: String,
}

impl CachedTrackAnalysis {
    #[must_use]
    pub fn into_analysis(self) -> TrackAnalysis {
        self.analysis
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub struct SyncAnalysisFixtures {
    resources: SyncFixtureResources,
    coordination: FixtureCache,
    analysis_root: PathBuf,
    beat_config: BeatAnalysisConfig<PlaybackResamplerBackend>,
    fingerprint: String,
}

impl SyncAnalysisFixtures {
    pub fn production(resources: &SyncFixtureResources) -> SyncFixtureResult<Self> {
        let beat_config = BeatAnalysisConfig::<PlaybackResamplerBackend>::default();
        let beat = beat_config.cache_tag().ok_or_else(|| {
            SyncFixtureError::InvalidConfig(
                "the production beat analyzer is not compiled into this test binary".to_owned(),
            )
        })?;
        let fingerprint = format!(
            "analysis-build={};wave=native:max{ANALYSIS_BUCKETS};beat={beat};decoder=default",
            env!("KITHARA_SYNC_ANALYSIS_BUILD"),
        );
        let cache_root = fixture_cache_dir();
        Ok(Self {
            resources: resources.clone(),
            coordination: FixtureCache::from_dir(Some(cache_root.clone())),
            analysis_root: cache_root.join("sync-analysis"),
            beat_config,
            fingerprint,
        })
    }

    pub async fn analyze(
        &self,
        master: &CancelToken,
        track: &SyncTrackFixture,
    ) -> SyncFixtureResult<CachedTrackAnalysis> {
        let key = analysis_key(track, &self.fingerprint);
        let key_hex = hex_digest(&key);
        if let Some(analysis) = self.load_cached(key, track.media()).await? {
            return Ok(CachedTrackAnalysis {
                analysis,
                key: key_hex,
            });
        }

        let config = analysis_config(track, &self.resources);
        let mut runner = TrackAnalysisRunner::new(
            master,
            ANALYSIS_BUCKETS,
            self.beat_config.clone(),
            self.resources.pcm_pool().clone(),
        );
        if !runner.is_active() {
            return Err(SyncFixtureError::IncompleteAnalysis {
                media: track.media.clone(),
                detail: "the production analysis runner is inactive".to_owned(),
            });
        }
        let mut updates = runner.analyze(config);
        let mut last = None;
        while updates.changed().await.is_ok() {
            last = updates.borrow().clone();
        }
        let analysis = last.ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: track.media.clone(),
            detail: "the production runner emitted no result".to_owned(),
        })?;
        validate_complete_analysis(&analysis, track.media())?;

        let content_source = track.content_source.clone();
        let expected_content_digest = track.content_digest;
        let content_resources = self.resources.clone();
        let observed_content_digest = blocking("verify analyzed track content", move || {
            content_source.digest(&content_resources)
        })
        .await?;
        if observed_content_digest != expected_content_digest {
            return Err(SyncFixtureError::InvalidConfig(format!(
                "'{}' changed while its analysis was running",
                track.media(),
            )));
        }

        let bytes = encode_analysis(&analysis, &key)?;
        let analysis = self.publish_cached(key, track.media(), bytes).await?;

        Ok(CachedTrackAnalysis {
            analysis,
            key: key_hex,
        })
    }

    async fn load_cached(
        &self,
        key: [u8; 32],
        media: &str,
    ) -> SyncFixtureResult<Option<TrackAnalysis>> {
        let resources = self.resources.clone();
        let coordination = self.coordination.clone();
        let analysis_root = self.analysis_root.clone();
        let media = media.to_owned();
        blocking("load sync analysis asset", move || {
            load_analysis_cache(&resources, &coordination, &analysis_root, &key, &media)
        })
        .await
    }

    async fn publish_cached(
        &self,
        key: [u8; 32],
        media: &str,
        bytes: Vec<u8>,
    ) -> SyncFixtureResult<TrackAnalysis> {
        let resources = self.resources.clone();
        let coordination = self.coordination.clone();
        let analysis_root = self.analysis_root.clone();
        let media = media.to_owned();
        blocking("publish sync analysis asset", move || {
            publish_analysis_cache(
                &resources,
                &coordination,
                &analysis_root,
                &key,
                &media,
                &bytes,
            )
        })
        .await
    }
}

fn analysis_config(track: &SyncTrackFixture, resources: &SyncFixtureResources) -> ResourceConfig {
    let builder = ResourceConfig::for_src(track.source.clone())
        .store(resources.store().clone())
        .byte_pool(resources.byte_pool().clone())
        .pcm_pool(resources.pcm_pool().clone());
    match track.profile {
        AnalysisProfile::Progressive => builder.build(),
        AnalysisProfile::Hls { variant } => builder
            .initial_abr_mode(AbrMode::manual(variant as usize))
            .build(),
    }
}

fn analysis_resource(
    store: &AssetStore,
    source: &AssetSource,
    key: &[u8; 32],
) -> SyncFixtureResult<ResourceKey> {
    let scope = store
        .scope::<()>(source)
        .map_err(|error| SyncFixtureError::AssetScope {
            operation: "create analysis scope",
            source: Box::new(source.clone()),
            error: Box::new(error),
        })?;
    scope
        .key(&AssetResource::Named {
            namespace: ANALYSIS_NAMESPACE.to_owned(),
            name: format!("{}.analysis", hex_digest(key)),
        })
        .map_err(|error| SyncFixtureError::AssetScope {
            operation: "create analysis resource",
            source: Box::new(source.clone()),
            error: Box::new(error),
        })
}

fn analysis_cache_root(base: &Path, key: &[u8; 32]) -> PathBuf {
    base.join(hex_digest(key))
}

fn load_analysis_cache(
    resources: &SyncFixtureResources,
    coordination: &FixtureCache,
    analysis_root: &Path,
    key: &[u8; 32],
    media: &str,
) -> SyncFixtureResult<Option<TrackAnalysis>> {
    let key_hex = hex_digest(key);
    let _entry = coordination
        .lock_entry(ANALYSIS_LOCK_DOMAIN, key)
        .map_err(|error| SyncFixtureError::AnalysisLock {
            key: key_hex,
            error,
        })?;
    let root = analysis_cache_root(analysis_root, key);
    let store = AssetStore::builder()
        .backend(StorageBackend::Disk { root: root.clone() })
        .pool(resources.byte_pool().clone())
        .build();
    let resource = analysis_resource(&store, &AssetSource::Local { path: root }, key)?;
    read_analysis_resource(resources, &store, &resource, |bytes| {
        decode_analysis(bytes, key, media)
    })
}

fn publish_analysis_cache(
    resources: &SyncFixtureResources,
    coordination: &FixtureCache,
    analysis_root: &Path,
    key: &[u8; 32],
    media: &str,
    bytes: &[u8],
) -> SyncFixtureResult<TrackAnalysis> {
    let key_hex = hex_digest(key);
    let _entry = coordination
        .lock_entry(ANALYSIS_LOCK_DOMAIN, key)
        .map_err(|error| SyncFixtureError::AnalysisLock {
            key: key_hex.clone(),
            error,
        })?;
    let root = analysis_cache_root(analysis_root, key);
    let store = AssetStore::builder()
        .backend(StorageBackend::Disk { root: root.clone() })
        .pool(resources.byte_pool().clone())
        .build();
    let resource = analysis_resource(&store, &AssetSource::Local { path: root }, key)?;
    if let Some(analysis) = read_analysis_resource(resources, &store, &resource, |stored| {
        decode_analysis(stored, key, media)
    })? {
        return Ok(analysis);
    }

    write_analysis_resource(&store, &resource, bytes)?;
    read_analysis_resource(resources, &store, &resource, |stored| {
        if stored != bytes {
            return Err(SyncFixtureError::CacheStore(key_hex.clone()));
        }
        decode_analysis(stored, key, media)
    })?
    .ok_or(SyncFixtureError::CacheStore(key_hex))
}

fn read_analysis_resource<T>(
    resources: &SyncFixtureResources,
    store: &AssetStore,
    resource: &ResourceKey,
    consume: impl FnOnce(&[u8]) -> SyncFixtureResult<T>,
) -> SyncFixtureResult<Option<T>> {
    let state = store
        .resource_state(resource)
        .map_err(|error| analysis_asset_error("inspect", resource, error))?;
    let final_len = match state {
        AssetResourceState::Missing => return Ok(None),
        AssetResourceState::Committed {
            final_len: Some(final_len),
        } => final_len,
        AssetResourceState::Committed { final_len: None } => {
            return Err(SyncFixtureError::AnalysisAssetLengthUnknown {
                resource: resource.clone(),
            });
        }
        state => {
            return Err(SyncFixtureError::AnalysisAssetState {
                resource: resource.clone(),
                state,
            });
        }
    };
    let len = usize::try_from(final_len).map_err(|_| SyncFixtureError::AnalysisAssetTooLarge {
        resource: resource.clone(),
        len: final_len,
        max: MAX_ANALYSIS_PAYLOAD_BYTES,
    })?;
    if len > MAX_ANALYSIS_PAYLOAD_BYTES {
        return Err(SyncFixtureError::AnalysisAssetTooLarge {
            resource: resource.clone(),
            len: final_len,
            max: MAX_ANALYSIS_PAYLOAD_BYTES,
        });
    }

    let reader = store
        .open_resource(resource, None)
        .map_err(|error| analysis_asset_error("open", resource, error))?;
    let mut bytes = resources.byte_pool().get();
    bytes
        .ensure_len(len)
        .map_err(|error| SyncFixtureError::AnalysisAssetBuffer {
            resource: resource.clone(),
            len,
            error,
        })?;
    let mut actual = 0_usize;
    while actual < len {
        let offset =
            u64::try_from(actual).map_err(|_| SyncFixtureError::AnalysisAssetTooLarge {
                resource: resource.clone(),
                len: final_len,
                max: MAX_ANALYSIS_PAYLOAD_BYTES,
            })?;
        let read = reader
            .read_at(offset, &mut bytes[actual..len])
            .map_err(AssetsError::from)
            .map_err(|error| analysis_asset_error("read", resource, error))?;
        if read == 0 {
            return Err(SyncFixtureError::AnalysisAssetTruncated {
                resource: resource.clone(),
                expected: len,
                actual,
            });
        }
        actual =
            actual
                .checked_add(read)
                .ok_or_else(|| SyncFixtureError::AnalysisAssetTooLarge {
                    resource: resource.clone(),
                    len: final_len,
                    max: MAX_ANALYSIS_PAYLOAD_BYTES,
                })?;
    }
    consume(&bytes[..len]).map(Some)
}

fn write_analysis_resource(
    store: &AssetStore,
    resource: &ResourceKey,
    bytes: &[u8],
) -> SyncFixtureResult<()> {
    write_new_resource(store, resource, bytes)
        .map_err(|error| analysis_asset_error("write", resource, error))?;
    Ok(())
}

fn analysis_asset_error(
    operation: &'static str,
    resource: &ResourceKey,
    error: AssetsError,
) -> SyncFixtureError {
    SyncFixtureError::AnalysisAsset {
        operation,
        resource: resource.clone(),
        error: Box::new(error),
    }
}

fn analysis_key(track: &SyncTrackFixture, fingerprint: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ANALYSIS_KEY_MAGIC);
    hasher.update(ANALYSIS_VERSION.to_le_bytes());
    hasher.update([track.profile.tag()]);
    hasher.update(track.profile.variant().to_le_bytes());
    hasher.update(track.content_digest);
    hasher.update((ANALYSIS_BUCKETS as u64).to_le_bytes());
    hasher.update((fingerprint.len() as u64).to_le_bytes());
    hasher.update(fingerprint.as_bytes());
    hasher.finalize().into()
}

fn validate_complete_analysis(analysis: &TrackAnalysis, media: &str) -> SyncFixtureResult<()> {
    if analysis.source_frames() == 0 {
        return incomplete(media, "decoded source frame count is zero");
    }
    analysis
        .source_sample_rate()
        .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: "source sample rate is absent".to_owned(),
        })?;
    let waveform = analysis
        .waveform()
        .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: "waveform is absent".to_owned(),
        })?;
    if waveform.is_empty() {
        return incomplete(media, "waveform is empty");
    }
    let beat = analysis
        .beat()
        .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: "beat grid is absent".to_owned(),
        })?;
    if beat.beats().len() < 2 {
        return incomplete(media, "beat grid has fewer than two markers");
    }
    analysis_map(analysis, media)?;
    Ok(())
}

pub fn analysis_map(analysis: &TrackAnalysis, media: &str) -> SyncFixtureResult<BeatMapSnapshot> {
    let source_rate =
        analysis
            .source_sample_rate()
            .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
                media: media.to_owned(),
                detail: "source sample rate is absent".to_owned(),
            })?;
    let grid = analysis
        .beat()
        .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: "beat grid is absent".to_owned(),
        })?;
    let axis = MapAxis::Asset(AssetAxis::new(source_rate, analysis.source_frames()));
    let uncertainty =
        FrameUncertainty::new(0.0).map_err(|error| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: format!("exact beat uncertainty is invalid: {error}"),
        })?;
    let meter = inferred_meter(grid, uncertainty);
    let segments = grid
        .beats()
        .windows(2)
        .enumerate()
        .map(|(ordinal, pair)| {
            let ordinal =
                i64::try_from(ordinal).map_err(|error| SyncFixtureError::IncompleteAnalysis {
                    media: media.to_owned(),
                    detail: format!("beat ordinal is not representable: {error}"),
                })?;
            let start = beat_marker(pair[0], ordinal, uncertainty, media)?;
            let end = beat_marker(pair[1], ordinal.saturating_add(1), uncertainty, media)?;
            MapSegment::new(
                start,
                end,
                SegmentFacts::new(BeatEvidence::Observed, uncertainty, meter),
            )
            .map_err(|error| SyncFixtureError::IncompleteAnalysis {
                media: media.to_owned(),
                detail: format!("beat segment is unusable: {error}"),
            })
        })
        .collect::<SyncFixtureResult<Vec<_>>>()?;
    let segments =
        SegmentSet::new(axis, segments).map_err(|error| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: format!("beat map topology is unusable: {error}"),
        })?;
    let id = BeatMapId::allocate().map_err(|error| SyncFixtureError::IncompleteAnalysis {
        media: media.to_owned(),
        detail: format!("beat map identity is unavailable: {error}"),
    })?;
    BeatMapSnapshot::initial(id, MapState::Complete, segments).map_err(|error| {
        SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: format!("beat map snapshot is unusable: {error}"),
        }
    })
}

pub fn unavailable_analysis_map(
    analysis: &TrackAnalysis,
    media: &str,
) -> SyncFixtureResult<BeatMapSnapshot> {
    let source_rate =
        analysis
            .source_sample_rate()
            .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
                media: media.to_owned(),
                detail: "source sample rate is absent".to_owned(),
            })?;
    let axis = MapAxis::Asset(AssetAxis::new(source_rate, analysis.source_frames()));
    let id = BeatMapId::allocate().map_err(|error| SyncFixtureError::IncompleteAnalysis {
        media: media.to_owned(),
        detail: format!("beat map identity is unavailable: {error}"),
    })?;
    Ok(BeatMapSnapshot::unavailable(id, axis))
}

fn beat_marker(
    frame: u64,
    ordinal: i64,
    uncertainty: FrameUncertainty,
    media: &str,
) -> SyncFixtureResult<BeatMarker> {
    let frame = frame
        .to_f64()
        .ok_or_else(|| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: "beat frame is not exactly representable".to_owned(),
        })?;
    let position =
        AssetFrame::new(frame).map_err(|error| SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: format!("beat frame is invalid: {error}"),
        })?;
    Ok(BeatMarker::new(
        position.into(),
        Some(BeatOrdinal::new(ordinal)),
        BeatEvidence::Observed,
        uncertainty,
    ))
}

fn inferred_meter(grid: &BeatGrid, uncertainty: FrameUncertainty) -> Option<MeterFacts> {
    let ordinals = grid
        .downbeats()
        .iter()
        .filter_map(|downbeat| grid.beats().binary_search(downbeat).ok())
        .collect::<Vec<_>>();
    let beats_per_bar = ordinals
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .next()?;
    if beats_per_bar == 0
        || ordinals
            .windows(2)
            .any(|pair| pair[1].saturating_sub(pair[0]) != beats_per_bar)
    {
        return None;
    }
    let beats_per_bar = u16::try_from(beats_per_bar).ok()?;
    let downbeat = i64::try_from(*ordinals.first()?).ok()?;
    Meter::with_downbeat(beats_per_bar, BeatOrdinal::new(downbeat))
        .ok()
        .map(|meter| MeterFacts::new(meter, BeatEvidence::Observed, uncertainty))
}

fn incomplete<T>(media: &str, detail: &str) -> SyncFixtureResult<T> {
    Err(SyncFixtureError::IncompleteAnalysis {
        media: media.to_owned(),
        detail: detail.to_owned(),
    })
}

fn encode_analysis(analysis: &TrackAnalysis, key: &[u8; 32]) -> SyncFixtureResult<Vec<u8>> {
    validate_complete_analysis(analysis, "cache payload")?;
    let waveform = Vec::<u8>::from(analysis.waveform().ok_or_else(|| {
        SyncFixtureError::CorruptCache("waveform disappeared during encoding".to_owned())
    })?);
    let beat = Vec::<u8>::from(analysis.beat().ok_or_else(|| {
        SyncFixtureError::CorruptCache("beat grid disappeared during encoding".to_owned())
    })?);
    let source_rate = analysis.source_sample_rate().ok_or_else(|| {
        SyncFixtureError::CorruptCache("source rate disappeared during encoding".to_owned())
    })?;
    let mut output = Vec::with_capacity(
        4 + 4 + key.len() + 8 + waveform.len() + 8 + beat.len() + 8 + 4 + CACHE_CHECKSUM_BYTES,
    );
    output.extend_from_slice(ANALYSIS_MAGIC);
    output.extend_from_slice(&ANALYSIS_VERSION.to_le_bytes());
    output.extend_from_slice(key);
    write_section(&mut output, &waveform)?;
    write_section(&mut output, &beat)?;
    output.extend_from_slice(&analysis.source_frames().to_le_bytes());
    output.extend_from_slice(&source_rate.get().to_le_bytes());
    let checksum = Sha256::digest(&output);
    output.extend_from_slice(&checksum);
    Ok(output)
}

fn decode_analysis(
    bytes: &[u8],
    expected_key: &[u8; 32],
    media: &str,
) -> SyncFixtureResult<TrackAnalysis> {
    if bytes.len() > MAX_ANALYSIS_PAYLOAD_BYTES {
        return Err(SyncFixtureError::CorruptCache(format!(
            "payload has {} bytes; maximum is {MAX_ANALYSIS_PAYLOAD_BYTES}",
            bytes.len(),
        )));
    }
    let body_len = bytes
        .len()
        .checked_sub(CACHE_CHECKSUM_BYTES)
        .ok_or_else(|| SyncFixtureError::CorruptCache("payload is truncated".to_owned()))?;
    let (body, stored_checksum) = bytes.split_at(body_len);
    let observed_checksum = Sha256::digest(body);
    if &observed_checksum[..] != stored_checksum {
        return Err(SyncFixtureError::CorruptCache(
            "checksum mismatch".to_owned(),
        ));
    }

    let mut cursor = 0;
    if read_slice(body, &mut cursor, ANALYSIS_MAGIC.len())? != ANALYSIS_MAGIC {
        return Err(SyncFixtureError::CorruptCache("magic mismatch".to_owned()));
    }
    let version = read_u32(body, &mut cursor)?;
    if version != ANALYSIS_VERSION {
        return Err(SyncFixtureError::CorruptCache(format!(
            "version {version} != {ANALYSIS_VERSION}"
        )));
    }
    let stored_key = read_slice(body, &mut cursor, expected_key.len())?;
    if stored_key != expected_key {
        return Err(SyncFixtureError::CorruptCache(
            "analysis key mismatch".to_owned(),
        ));
    }
    let waveform_bytes = read_section(body, &mut cursor)?;
    let beat_bytes = read_section(body, &mut cursor)?;
    if waveform_bytes.is_empty() || beat_bytes.is_empty() {
        return Err(SyncFixtureError::CorruptCache(
            "complete analysis requires waveform and beat sections".to_owned(),
        ));
    }
    let source_frames = read_u64(body, &mut cursor)?;
    let source_rate = NonZeroU32::new(read_u32(body, &mut cursor)?)
        .ok_or_else(|| SyncFixtureError::CorruptCache("source sample rate is zero".to_owned()))?;
    if cursor != body.len() {
        return Err(SyncFixtureError::CorruptCache(
            "payload has trailing bytes".to_owned(),
        ));
    }
    let waveform = Waveform::try_from(waveform_bytes)
        .map_err(|error| SyncFixtureError::CorruptCache(format!("waveform: {error}")))?;
    let beat = BeatGrid::try_from(beat_bytes)
        .map_err(|error| SyncFixtureError::CorruptCache(format!("beat grid: {error}")))?;
    let analysis =
        TrackAnalysis::with_source_rate(Some(beat), Some(waveform), source_frames, source_rate);
    validate_complete_analysis(&analysis, media)?;
    Ok(analysis)
}

fn write_section(output: &mut Vec<u8>, section: &[u8]) -> SyncFixtureResult<()> {
    let len = u64::try_from(section.len())
        .map_err(|_| SyncFixtureError::CorruptCache("analysis section is too large".to_owned()))?;
    output.extend_from_slice(&len.to_le_bytes());
    output.extend_from_slice(section);
    Ok(())
}

fn read_section<'a>(bytes: &'a [u8], cursor: &mut usize) -> SyncFixtureResult<&'a [u8]> {
    let len = usize::try_from(read_u64(bytes, cursor)?)
        .map_err(|_| SyncFixtureError::CorruptCache("section is too large".to_owned()))?;
    read_slice(bytes, cursor, len)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> SyncFixtureResult<u32> {
    let raw = read_array::<4>(bytes, cursor)?;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> SyncFixtureResult<u64> {
    let raw = read_array::<8>(bytes, cursor)?;
    Ok(u64::from_le_bytes(raw))
}

fn read_array<const N: usize>(bytes: &[u8], cursor: &mut usize) -> SyncFixtureResult<[u8; N]> {
    let raw = read_slice(bytes, cursor, N)?;
    let mut output = [0; N];
    output.copy_from_slice(raw);
    Ok(output)
}

fn read_slice<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> SyncFixtureResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| SyncFixtureError::CorruptCache("cursor overflow".to_owned()))?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or_else(|| SyncFixtureError::CorruptCache("payload is truncated".to_owned()))?;
    *cursor = end;
    Ok(slice)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ::kithara::{
        assets::AcquisitionResult,
        audio::{Bucket, GridSegment},
        play::ResourceSrc,
    };
    use kithara_test_utils::kithara;
    use url::Url;

    use super::*;
    use crate::sync_fixture::{library::local_fixture, test_resources};

    fn complete_analysis() -> TrackAnalysis {
        let rate = NonZeroU32::new(48_000).expect("fixture rate");
        let beat = BeatGrid::new(
            120.0,
            vec![0, 24_000, 48_000, 72_000],
            vec![0],
            vec![GridSegment::new(0, 96_000, 1.0)],
        );
        let waveform = Waveform::from(vec![Bucket::new(0.2, 0.3, 0.4), Bucket::new(0.5, 0.6, 0.7)]);
        TrackAnalysis::with_source_rate(Some(beat), Some(waveform), 96_000, rate)
    }

    fn competing_analysis() -> TrackAnalysis {
        let rate = NonZeroU32::new(48_000).expect("fixture rate");
        let beat = BeatGrid::new(
            120.0,
            vec![0, 24_000, 48_000, 72_000],
            vec![0],
            vec![GridSegment::new(0, 96_000, 1.0)],
        );
        let waveform = Waveform::from(vec![Bucket::new(0.1, 0.8, 0.9)]);
        TrackAnalysis::with_source_rate(Some(beat), Some(waveform), 96_000, rate)
    }

    #[kithara::test]
    fn equal_case_keys_still_use_isolated_resource_owners() {
        let root = tempfile::tempdir().expect("resource isolation temp dir");
        let first = test_resources("sync-fixture-unit-owner-isolation");
        let second = test_resources("sync-fixture-unit-owner-isolation");
        let source = AssetSource::Local {
            path: root.path().join("track.mp3"),
        };
        let key = [11; 32];
        let first_resource =
            analysis_resource(first.store(), &source, &key).expect("first resource");
        let second_resource =
            analysis_resource(second.store(), &source, &key).expect("second resource");

        assert_eq!(first_resource, second_resource);
        write_analysis_resource(first.store(), &first_resource, b"first")
            .expect("write first owner store");
        assert_eq!(
            second
                .store()
                .resource_state(&second_resource)
                .expect("inspect second owner store"),
            AssetResourceState::Missing,
        );
    }

    #[kithara::test]
    fn cached_analysis_bytes_preserve_complete_snapshot() {
        let analysis = complete_analysis();
        let key = [7; 32];
        let bytes = encode_analysis(&analysis, &key).expect("encode analysis");
        let decoded = decode_analysis(&bytes, &key, "fixture").expect("decode analysis");

        assert_eq!(decoded.source_frames(), analysis.source_frames());
        assert_eq!(decoded.source_sample_rate(), analysis.source_sample_rate());
        assert_eq!(
            decoded.beat().map(Vec::<u8>::from),
            analysis.beat().map(Vec::<u8>::from),
        );
        assert_eq!(
            decoded.waveform().map(Vec::<u8>::from),
            analysis.waveform().map(Vec::<u8>::from),
        );
    }

    #[kithara::test]
    fn cached_analysis_round_trips_through_the_asset_store() {
        let root = tempfile::tempdir().expect("analysis store temp dir");
        let resources = test_resources("sync-fixture-unit-analysis-roundtrip");
        let source = AssetSource::Local {
            path: root.path().join("track.mp3"),
        };
        let analysis = complete_analysis();
        let key = [7; 32];
        let resource =
            analysis_resource(resources.store(), &source, &key).expect("analysis resource");
        let encoded = encode_analysis(&analysis, &key).expect("encode analysis");

        write_analysis_resource(resources.store(), &resource, &encoded).expect("store analysis");
        let stored = read_analysis_resource(&resources, resources.store(), &resource, |stored| {
            assert_eq!(stored, encoded.as_slice());
            decode_analysis(stored, &key, "fixture")
        })
        .expect("read analysis")
        .expect("committed analysis");

        assert_eq!(stored.source_frames(), analysis.source_frames());
    }

    #[kithara::test]
    fn analysis_cache_rechecks_across_independent_owners_and_never_overwrites_winner() {
        let cache = tempfile::tempdir().expect("analysis cache temp dir");
        let coordination = FixtureCache::from_dir(Some(cache.path().join("locks")));
        let analysis_root = cache.path().join("payloads");
        let first_owner = test_resources("sync-fixture-unit-analysis-first-owner");
        let competing_owner = test_resources("sync-fixture-unit-analysis-competing-owner");
        let fresh_owner = test_resources("sync-fixture-unit-analysis-fresh-owner");
        let key = [13; 32];
        let first = complete_analysis();
        let competing = competing_analysis();
        let first_bytes = encode_analysis(&first, &key).expect("encode first analysis");
        let competing_bytes = encode_analysis(&competing, &key).expect("encode competing analysis");

        assert!(
            load_analysis_cache(&first_owner, &coordination, &analysis_root, &key, "fixture",)
                .expect("load cold analysis cache")
                .is_none()
        );
        let published = publish_analysis_cache(
            &first_owner,
            &coordination,
            &analysis_root,
            &key,
            "fixture",
            &first_bytes,
        )
        .expect("publish first analysis");
        let observed_by_competitor = publish_analysis_cache(
            &competing_owner,
            &coordination,
            &analysis_root,
            &key,
            "fixture",
            &competing_bytes,
        )
        .expect("competing publisher reuses the committed winner");
        let observed_by_fresh_owner =
            load_analysis_cache(&fresh_owner, &coordination, &analysis_root, &key, "fixture")
                .expect("load analysis through a fresh store")
                .expect("analysis remains committed");

        let expected_waveform = published.waveform().map(Vec::<u8>::from);
        assert_ne!(
            expected_waveform,
            competing.waveform().map(Vec::<u8>::from),
            "the fixture must prove that the competing payload differs",
        );
        assert_eq!(
            observed_by_competitor.waveform().map(Vec::<u8>::from),
            expected_waveform,
        );
        assert_eq!(
            observed_by_fresh_owner.waveform().map(Vec::<u8>::from),
            expected_waveform,
        );
    }

    #[kithara::test]
    fn active_analysis_resource_is_not_treated_as_a_cache_miss() {
        let root = tempfile::tempdir().expect("analysis store temp dir");
        let resources = test_resources("sync-fixture-unit-analysis-active");
        let source = AssetSource::Local {
            path: root.path().join("track.mp3"),
        };
        let resource =
            analysis_resource(resources.store(), &source, &[3; 32]).expect("analysis resource");
        let AcquisitionResult::Pending(_writer) = resources
            .store()
            .acquire_resource(&resource, None)
            .expect("acquire active analysis resource")
        else {
            panic!("new analysis resource must be pending");
        };

        let error = read_analysis_resource(&resources, resources.store(), &resource, |_| Ok(()))
            .expect_err("active analysis resource must fail closed");

        assert!(matches!(
            error,
            SyncFixtureError::AnalysisAssetState {
                state: AssetResourceState::Active,
                ..
            }
        ));
    }

    #[kithara::test]
    fn corrupt_committed_analysis_is_not_treated_as_a_cache_miss() {
        let root = tempfile::tempdir().expect("analysis store temp dir");
        let resources = test_resources("sync-fixture-unit-analysis-corrupt");
        let source = AssetSource::Local {
            path: root.path().join("track.mp3"),
        };
        let key = [5; 32];
        let resource =
            analysis_resource(resources.store(), &source, &key).expect("analysis resource");
        write_analysis_resource(resources.store(), &resource, b"corrupt")
            .expect("store corrupt analysis");

        let error = read_analysis_resource(&resources, resources.store(), &resource, |stored| {
            decode_analysis(stored, &key, "fixture")
        })
        .expect_err("corrupt committed analysis must fail closed");

        assert!(matches!(error, SyncFixtureError::CorruptCache(_)));
    }

    #[kithara::test]
    fn analysis_resources_are_stable_and_content_addressed() {
        let root = tempfile::tempdir().expect("analysis store temp dir");
        let resources = test_resources("sync-fixture-unit-analysis-resource-identity");
        let source = AssetSource::Local {
            path: root.path().join("track.mp3"),
        };

        let first =
            analysis_resource(resources.store(), &source, &[1; 32]).expect("first resource");
        let repeated =
            analysis_resource(resources.store(), &source, &[1; 32]).expect("repeated resource");
        let other =
            analysis_resource(resources.store(), &source, &[2; 32]).expect("other resource");

        assert_eq!(first, repeated);
        assert_ne!(first, other);
        let cache_root = Path::new("analysis-cache");
        assert_eq!(
            analysis_cache_root(cache_root, &[1; 32]),
            analysis_cache_root(cache_root, &[1; 32]),
        );
        assert_ne!(
            analysis_cache_root(cache_root, &[1; 32]),
            analysis_cache_root(cache_root, &[2; 32]),
        );
    }

    #[kithara::test]
    fn cached_analysis_rejects_corruption_truncation_and_wrong_key() {
        let analysis = complete_analysis();
        let key = [9; 32];
        let bytes = encode_analysis(&analysis, &key).expect("encode analysis");

        let mut corrupt = bytes.clone();
        corrupt[12] ^= 1;
        assert!(decode_analysis(&corrupt, &key, "fixture").is_err());
        assert!(decode_analysis(&bytes[..bytes.len() - 1], &key, "fixture").is_err());
        assert!(decode_analysis(&bytes, &[8; 32], "fixture").is_err());
    }

    #[kithara::test]
    fn analysis_key_changes_with_content_profile_and_configuration() {
        let root = tempfile::tempdir().expect("key temp dir");
        let resources = test_resources("sync-fixture-unit-analysis-key");
        let path = root.path().join("track.mp3");
        let other_path = root.path().join("other.mp3");
        fs::write(&path, b"track").expect("write track");
        fs::write(&other_path, b"other").expect("write other track");
        let progressive = local_fixture(&resources, &path).expect("progressive fixture");
        let other_content = local_fixture(&resources, &other_path).expect("other fixture");
        let mut other_server = progressive.clone();
        other_server.source = ResourceSrc::Url(
            Url::parse("http://127.0.0.1:54321/assets/track.mp3").expect("fixture URL"),
        );
        let mut hls = progressive.clone();
        hls.profile = AnalysisProfile::Hls { variant: 0 };

        assert_eq!(
            analysis_key(&progressive, "config-a"),
            analysis_key(&other_server, "config-a"),
            "ephemeral local-server URLs must not invalidate content-addressed analysis",
        );
        assert_ne!(
            analysis_key(&progressive, "config-a"),
            analysis_key(&progressive, "config-b"),
        );
        assert_ne!(
            analysis_key(&progressive, "config-a"),
            analysis_key(&other_content, "config-a"),
        );
        assert_ne!(
            analysis_key(&progressive, "config-a"),
            analysis_key(&hls, "config-a"),
        );
    }
}
