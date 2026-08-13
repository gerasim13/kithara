use std::{
    ffi::{OsStr, OsString},
    fmt::Write as _,
    fs::{self, File},
    io::{self, Read, Write as _},
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use kithara::{
    audio::{BeatGrid, TrackBeatMap, Waveform, analysis::BeatAnalysisConfig},
    bufpool::{BytePool, PcmPool},
    hls::AbrMode,
    platform::{CancelToken, sync::Arc, time::SystemTime, tokio::task},
    play::{PlaybackResamplerBackend, ResourceConfig, ResourceSrc, TrackAnalysis},
};
use kithara_app::waveform::TrackAnalysisRunner;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::warn;
#[cfg(test)]
use url::Url;

use crate::{TestServerHelper, fixture_cache::FixtureCache, memory_asset_store};

const LIBRARY_ENV: &str = "KITHARA_SYNC_LIBRARY";
const LIBRARY_SEED_ENV: &str = "KITHARA_SYNC_LIBRARY_SEED";
const LIBRARY_TRACK_A_ENV: &str = "KITHARA_SYNC_LIBRARY_TRACK_A";
const LIBRARY_TRACK_B_ENV: &str = "KITHARA_SYNC_LIBRARY_TRACK_B";

const DEFAULT_LIBRARY_SEED: u64 = 0x4b49_5448_4152_4101;
const ANALYSIS_BUCKETS: usize = 96_000;

const ANALYSIS_DOMAIN: &str = "sync-track-analysis-v1";
const ANALYSIS_KEY_MAGIC: &[u8] = b"kithara-sync-analysis-key\0";
const ANALYSIS_MAGIC: &[u8; 4] = b"KSAN";
const ANALYSIS_VERSION: u32 = 1;
const CACHE_CHECKSUM_BYTES: usize = 32;
const FILE_DIGEST_MAGIC: &[u8] = b"kithara-sync-file\0";
const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const HLS_ANALYSIS_VARIANT: u32 = 0;
const MAX_ANALYSIS_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;
const PREPARED_ANALYSIS_KEY_MAGIC: &[u8] = b"kithara-prepared-sync-analysis\0";
const PREPARED_ANALYSIS_VERSION: u32 = 1;
const TREE_DIGEST_MAGIC: &[u8] = b"kithara-sync-tree\0";

pub type SyncFixtureResult<T> = Result<T, SyncFixtureError>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SyncFixtureError {
    #[error("invalid sync fixture configuration: {0}")]
    InvalidConfig(String),
    #[error("{operation} '{path}' failed: {error}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        error: io::Error,
    },
    #[error("sync fixture blocking task '{operation}' failed: {detail}")]
    Blocking {
        operation: &'static str,
        detail: String,
    },
    #[error("track analysis for '{media}' is incomplete: {detail}")]
    IncompleteAnalysis { media: String, detail: String },
    #[error("sync analysis cache is corrupt: {0}")]
    CorruptCache(String),
    #[error("sync analysis cache failed to persist key {0}")]
    CacheStore(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryMp3 {
    Silvercomet,
    Test,
}

impl RepositoryMp3 {
    const fn asset_name(self) -> &'static str {
        match self {
            Self::Silvercomet => "sync-silvercomet.mp3",
            Self::Test => "test.mp3",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AnalysisProfile {
    Progressive,
    Hls { variant: u32 },
}

impl AnalysisProfile {
    const fn tag(self) -> u8 {
        match self {
            Self::Progressive => 0,
            Self::Hls { .. } => 1,
        }
    }

    const fn variant(self) -> u32 {
        match self {
            Self::Progressive => u32::MAX,
            Self::Hls { variant } => variant,
        }
    }
}

#[derive(Clone, Debug)]
enum ContentSource {
    File(PathBuf),
    Tree(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedAnalysis {
    SilvercometHlsV0,
    SilvercometMp3,
    TestMp3,
}

impl PreparedAnalysis {
    const fn file_name(self) -> &'static str {
        match self {
            Self::SilvercometHlsV0 => "silvercomet-hls-v0.ksan",
            Self::SilvercometMp3 => "sync-silvercomet-mp3.ksan",
            Self::TestMp3 => "test-mp3.ksan",
        }
    }

    fn path(self) -> PathBuf {
        repository_assets_root()
            .join("sync-analysis")
            .join(self.file_name())
    }
}

impl ContentSource {
    fn digest(&self) -> SyncFixtureResult<[u8; 32]> {
        match self {
            Self::File(path) => digest_file(path),
            Self::Tree(path) => digest_tree(path),
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SyncTrackFixture {
    media: String,
    source: ResourceSrc,
    content_source: ContentSource,
    content_digest: [u8; 32],
    profile: AnalysisProfile,
    prepared_analysis: Option<PreparedAnalysis>,
}

impl SyncTrackFixture {
    #[must_use]
    pub fn media(&self) -> &str {
        &self.media
    }

    #[must_use]
    pub fn source(&self) -> &ResourceSrc {
        &self.source
    }

    #[cfg(test)]
    #[must_use]
    fn path(&self) -> Option<&Path> {
        match &self.source {
            ResourceSrc::Path(path) => Some(path),
            ResourceSrc::Url(_) => None,
        }
    }

    #[must_use]
    pub const fn is_hls(&self) -> bool {
        matches!(self.profile, AnalysisProfile::Hls { .. })
    }

    fn local(path: PathBuf, digest: [u8; 32]) -> Self {
        Self {
            media: path.display().to_string(),
            source: ResourceSrc::Path(path.clone()),
            content_source: ContentSource::File(path),
            content_digest: digest,
            profile: AnalysisProfile::Progressive,
            prepared_analysis: None,
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SyncFixturePair {
    deck_a: SyncTrackFixture,
    deck_b: SyncTrackFixture,
    library_seed: Option<u64>,
}

impl SyncFixturePair {
    #[must_use]
    pub fn deck_a(&self) -> &SyncTrackFixture {
        &self.deck_a
    }

    #[must_use]
    pub fn deck_b(&self) -> &SyncTrackFixture {
        &self.deck_b
    }

    #[must_use]
    pub const fn library_seed(&self) -> Option<u64> {
        self.library_seed
    }
}

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
    cache: FixtureCache,
    beat_config: BeatAnalysisConfig<PlaybackResamplerBackend>,
    fingerprint: String,
    prepared_fingerprint: String,
}

impl SyncAnalysisFixtures {
    pub fn production() -> SyncFixtureResult<Self> {
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
        let prepared_fingerprint =
            format!("wave=native:max{ANALYSIS_BUCKETS};beat={beat};decoder=default");
        Ok(Self {
            cache: FixtureCache::from_env(),
            beat_config,
            fingerprint,
            prepared_fingerprint,
        })
    }

    /// Load the checked-in complete analysis for this exact media identity.
    /// This fails closed without running the analyzer or consulting its cache.
    pub async fn load_prepared(
        &self,
        track: &SyncTrackFixture,
    ) -> SyncFixtureResult<CachedTrackAnalysis> {
        let prepared = track.prepared_analysis.ok_or_else(|| {
            SyncFixtureError::InvalidConfig(format!(
                "'{}' has no checked-in prepared analysis",
                track.media(),
            ))
        })?;
        let content_source = track.content_source.clone();
        let path = prepared.path();
        let input_path = path.clone();
        let (content_digest, bytes) = blocking("read prepared sync analysis", move || {
            let content_digest = content_source.digest()?;
            let bytes = fs::read(&input_path)
                .map_err(|error| io_error("read prepared sync analysis", &input_path, error))?;
            Ok((content_digest, bytes))
        })
        .await?;
        if content_digest != track.content_digest {
            return Err(SyncFixtureError::InvalidConfig(format!(
                "'{}' changed after its prepared-analysis identity was resolved",
                track.media(),
            )));
        }
        let key = prepared_analysis_key(track, &self.prepared_fingerprint);
        let analysis = decode_analysis(&bytes, &key, track.media())?;
        Ok(CachedTrackAnalysis {
            analysis,
            key: hex_digest(&key),
        })
    }

    /// Regenerate and atomically replace one checked-in analysis fixture.
    pub async fn write_prepared(
        &self,
        master: &CancelToken,
        track: &SyncTrackFixture,
    ) -> SyncFixtureResult<PathBuf> {
        let prepared = track.prepared_analysis.ok_or_else(|| {
            SyncFixtureError::InvalidConfig(format!(
                "'{}' has no checked-in prepared analysis target",
                track.media(),
            ))
        })?;
        let cached = self.analyze(master, track).await?;
        let key = prepared_analysis_key(track, &self.prepared_fingerprint);
        let bytes = encode_analysis(&cached.analysis, &key)?;
        decode_analysis(&bytes, &key, track.media())?;
        let path = prepared.path();
        let output = path.clone();
        blocking("write prepared sync analysis", move || {
            write_prepared_file(&output, &bytes)?;
            Ok(output)
        })
        .await
    }

    pub async fn analyze(
        &self,
        master: &CancelToken,
        track: &SyncTrackFixture,
    ) -> SyncFixtureResult<CachedTrackAnalysis> {
        let key = analysis_key(track, &self.fingerprint);
        let key_hex = hex_digest(&key);
        if let Some(analysis) = self.load_cached(&key, track.media()) {
            return Ok(CachedTrackAnalysis {
                analysis,
                key: key_hex,
            });
        }

        let cache = self.cache.clone();
        let lock_key = key;
        let _entry = blocking("lock sync analysis cache", move || {
            Ok(cache.lock_entry(ANALYSIS_DOMAIN, &lock_key))
        })
        .await?;

        if let Some(analysis) = self.load_cached(&key, track.media()) {
            return Ok(CachedTrackAnalysis {
                analysis,
                key: key_hex,
            });
        }

        let pcm_pool = PcmPool::default();
        let config = analysis_config(track, pcm_pool.clone());
        let mut runner =
            TrackAnalysisRunner::new(master, ANALYSIS_BUCKETS, self.beat_config.clone(), pcm_pool);
        if !runner.is_active() {
            return Err(SyncFixtureError::IncompleteAnalysis {
                media: track.media.clone(),
                detail: "the production analysis runner is inactive".to_owned(),
            });
        }
        let mut updates = runner.analyze(config, Arc::from(track.media.as_str()));
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
        let observed_content_digest = blocking("verify analyzed track content", move || {
            content_source.digest()
        })
        .await?;
        if observed_content_digest != expected_content_digest {
            return Err(SyncFixtureError::InvalidConfig(format!(
                "'{}' changed while its analysis was running",
                track.media(),
            )));
        }

        let bytes = encode_analysis(&analysis, &key)?;
        self.cache.store(ANALYSIS_DOMAIN, &key, &bytes);
        let persisted = self
            .cache
            .get(ANALYSIS_DOMAIN, &key)
            .filter(|stored| stored == &bytes)
            .ok_or_else(|| SyncFixtureError::CacheStore(key_hex.clone()))?;
        decode_analysis(&persisted, &key, track.media())?;

        Ok(CachedTrackAnalysis {
            analysis,
            key: key_hex,
        })
    }

    fn load_cached(&self, key: &[u8; 32], media: &str) -> Option<TrackAnalysis> {
        let bytes = self.cache.get(ANALYSIS_DOMAIN, key)?;
        match decode_analysis(&bytes, key, media) {
            Ok(analysis) => Some(analysis),
            Err(error) => {
                warn!(%error, media, "ignoring corrupt sync analysis cache entry");
                None
            }
        }
    }
}

#[derive(Clone, Debug)]
struct LibraryEnv {
    root: Option<OsString>,
    seed: Option<OsString>,
    track_a: Option<OsString>,
    track_b: Option<OsString>,
}

impl LibraryEnv {
    fn read() -> Self {
        Self {
            root: std::env::var_os(LIBRARY_ENV),
            seed: std::env::var_os(LIBRARY_SEED_ENV),
            track_a: std::env::var_os(LIBRARY_TRACK_A_ENV),
            track_b: std::env::var_os(LIBRARY_TRACK_B_ENV),
        }
    }
}

pub async fn library_pair_from_env() -> SyncFixtureResult<Option<SyncFixturePair>> {
    let values = LibraryEnv::read();
    if values.root.is_none() {
        return Ok(None);
    }
    blocking("resolve opt-in sync library", move || {
        library_pair_from_values(values)
    })
    .await
}

fn select_library_pair(root: &Path, seed: u64) -> SyncFixtureResult<SyncFixturePair> {
    let root = canonical_library_root(root)?;
    let mut tracks = Vec::new();
    collect_library_tracks(&root, &mut tracks)?;
    tracks.sort();
    tracks.dedup();
    if tracks.len() < 2 {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "sync music library '{}' has {} supported track(s); at least two are required",
            root.display(),
            tracks.len(),
        )));
    }

    let first_index = stable_index(seed, tracks.len());
    let deck_a = local_fixture(&tracks[first_index])?;
    let second_start = stable_index(mix_seed(seed), tracks.len() - 1);
    for offset in 0..tracks.len() - 1 {
        let slot = (second_start + offset) % (tracks.len() - 1);
        let second_index = if slot >= first_index { slot + 1 } else { slot };
        let deck_b = local_fixture(&tracks[second_index])?;
        if deck_a.content_digest != deck_b.content_digest {
            return Ok(SyncFixturePair {
                deck_a,
                deck_b,
                library_seed: Some(seed),
            });
        }
    }

    Err(SyncFixtureError::InvalidConfig(format!(
        "sync music library '{}' contains no two tracks with distinct content",
        root.display(),
    )))
}

pub async fn repository_mp3(
    server: &TestServerHelper,
    which: RepositoryMp3,
) -> SyncFixtureResult<SyncTrackFixture> {
    let asset_name = which.asset_name();
    let path = repository_assets_root().join(asset_name);
    let content_path = path.clone();
    let digest = blocking("hash repository MP3", move || digest_file(&content_path)).await?;
    Ok(SyncTrackFixture {
        media: format!("repo:{asset_name}"),
        source: ResourceSrc::Url(server.asset(asset_name)),
        content_source: ContentSource::File(path),
        content_digest: digest,
        profile: AnalysisProfile::Progressive,
        prepared_analysis: Some(match which {
            RepositoryMp3::Silvercomet => PreparedAnalysis::SilvercometMp3,
            RepositoryMp3::Test => PreparedAnalysis::TestMp3,
        }),
    })
}

pub async fn repository_mp3_pair(server: &TestServerHelper) -> SyncFixtureResult<SyncFixturePair> {
    let deck_a = repository_mp3(server, RepositoryMp3::Test).await?;
    let deck_b = repository_mp3(server, RepositoryMp3::Silvercomet).await?;
    if deck_a.content_digest == deck_b.content_digest {
        return Err(SyncFixtureError::InvalidConfig(
            "repository MP3 fixtures must contain different audio".to_owned(),
        ));
    }
    Ok(SyncFixturePair {
        deck_a,
        deck_b,
        library_seed: None,
    })
}

pub async fn silvercomet_hls(server: &TestServerHelper) -> SyncFixtureResult<SyncTrackFixture> {
    let path = repository_assets_root().join("hls");
    let content_path = path.clone();
    let digest = blocking("hash local Silvercomet HLS", move || {
        digest_tree(&content_path)
    })
    .await?;
    Ok(SyncTrackFixture {
        media: format!("repo:hls/master.m3u8#analysis-variant={HLS_ANALYSIS_VARIANT}"),
        source: ResourceSrc::Url(server.asset("hls/master.m3u8")),
        content_source: ContentSource::Tree(path),
        content_digest: digest,
        profile: AnalysisProfile::Hls {
            variant: HLS_ANALYSIS_VARIANT,
        },
        prepared_analysis: Some(PreparedAnalysis::SilvercometHlsV0),
    })
}

fn library_pair_from_values(values: LibraryEnv) -> SyncFixtureResult<Option<SyncFixturePair>> {
    let Some(root) = values.root else {
        return Ok(None);
    };
    if root.is_empty() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{LIBRARY_ENV} must not be empty"
        )));
    }
    let root = canonical_library_root(Path::new(&root))?;
    let seed = parse_library_seed(values.seed)?;
    match (values.track_a, values.track_b) {
        (Some(track_a), Some(track_b)) => {
            let track_a = resolve_library_track(&root, &track_a, LIBRARY_TRACK_A_ENV)?;
            let track_b = resolve_library_track(&root, &track_b, LIBRARY_TRACK_B_ENV)?;
            if track_a == track_b {
                return Err(SyncFixtureError::InvalidConfig(format!(
                    "{LIBRARY_TRACK_A_ENV} and {LIBRARY_TRACK_B_ENV} must name different tracks"
                )));
            }
            let deck_a = local_fixture(&track_a)?;
            let deck_b = local_fixture(&track_b)?;
            if deck_a.content_digest == deck_b.content_digest {
                return Err(SyncFixtureError::InvalidConfig(format!(
                    "{LIBRARY_TRACK_A_ENV} and {LIBRARY_TRACK_B_ENV} must contain different audio"
                )));
            }
            Ok(Some(SyncFixturePair {
                deck_a,
                deck_b,
                library_seed: Some(seed),
            }))
        }
        (None, None) => select_library_pair(&root, seed).map(Some),
        _ => Err(SyncFixtureError::InvalidConfig(format!(
            "{LIBRARY_TRACK_A_ENV} and {LIBRARY_TRACK_B_ENV} must either both be set or both be absent"
        ))),
    }
}

fn parse_library_seed(value: Option<OsString>) -> SyncFixtureResult<u64> {
    let Some(value) = value else {
        return Ok(DEFAULT_LIBRARY_SEED);
    };
    let value = value.into_string().map_err(|_| {
        SyncFixtureError::InvalidConfig(format!(
            "{LIBRARY_SEED_ENV} must be an unsigned 64-bit integer"
        ))
    })?;
    value.parse::<u64>().map_err(|_| {
        SyncFixtureError::InvalidConfig(format!(
            "{LIBRARY_SEED_ENV} must be an unsigned 64-bit integer"
        ))
    })
}

fn canonical_library_root(root: &Path) -> SyncFixtureResult<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{LIBRARY_ENV} must not be empty"
        )));
    }
    let canonical = canonicalize("canonicalize sync music library", root)?;
    if !canonical.is_dir() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "sync music library '{}' is not a directory",
            canonical.display(),
        )));
    }
    Ok(canonical)
}

fn resolve_library_track(
    root: &Path,
    configured: &OsStr,
    variable: &str,
) -> SyncFixtureResult<PathBuf> {
    if configured.is_empty() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{variable} must not be empty"
        )));
    }
    let configured = Path::new(configured);
    let candidate = if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        root.join(configured)
    };
    let canonical = canonicalize("canonicalize explicit sync track", &candidate)?;
    if !canonical.starts_with(root) {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{variable}='{}' is outside {LIBRARY_ENV}='{}'",
            canonical.display(),
            root.display(),
        )));
    }
    validate_audio_file(&canonical, variable)?;
    Ok(canonical)
}

fn local_fixture(path: &Path) -> SyncFixtureResult<SyncTrackFixture> {
    let canonical = canonicalize("canonicalize local sync track", path)?;
    validate_audio_file(&canonical, "sync track")?;
    let digest = digest_file(&canonical)?;
    Ok(SyncTrackFixture::local(canonical, digest))
}

fn validate_audio_file(path: &Path, label: &str) -> SyncFixtureResult<()> {
    if !path.is_file() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{label}='{}' is not a regular file",
            path.display(),
        )));
    }
    if !is_supported_audio(path) {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "{label}='{}' is not a supported audio file",
            path.display(),
        )));
    }
    Ok(())
}

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "aac" | "aif" | "aiff" | "flac" | "m4a" | "mp3" | "mp4" | "ogg" | "opus" | "wav"
            )
        })
}

fn repository_assets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets")
}

fn analysis_config(track: &SyncTrackFixture, pcm_pool: PcmPool) -> ResourceConfig {
    let builder = ResourceConfig::for_src(track.source.clone())
        .store(memory_asset_store())
        .byte_pool(BytePool::default())
        .pcm_pool(pcm_pool);
    match track.profile {
        AnalysisProfile::Progressive => builder.build(),
        AnalysisProfile::Hls { variant } => builder
            .initial_abr_mode(AbrMode::manual(variant as usize))
            .build(),
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

fn prepared_analysis_key(track: &SyncTrackFixture, fingerprint: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(PREPARED_ANALYSIS_KEY_MAGIC);
    hasher.update(PREPARED_ANALYSIS_VERSION.to_le_bytes());
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
    let source_rate =
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
    TrackBeatMap::new(analysis, source_rate).map_err(|error| {
        SyncFixtureError::IncompleteAnalysis {
            media: media.to_owned(),
            detail: format!("beat map is unusable: {error}"),
        }
    })?;
    Ok(())
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

fn write_prepared_file(path: &Path, bytes: &[u8]) -> SyncFixtureResult<()> {
    let parent = path.parent().ok_or_else(|| {
        SyncFixtureError::InvalidConfig(format!(
            "prepared analysis path '{}' has no parent",
            path.display(),
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| io_error("create prepared analysis directory", parent, error))?;
    let mut file = NamedTempFile::new_in(parent)
        .map_err(|error| io_error("create prepared analysis", parent, error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write prepared analysis", file.path(), error))?;
    file.as_file()
        .sync_all()
        .map_err(|error| io_error("sync prepared analysis", file.path(), error))?;
    persist_prepared_file(file, path)?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error("sync prepared analysis directory", parent, error))?;
    Ok(())
}

fn persist_prepared_file(file: NamedTempFile, path: &Path) -> SyncFixtureResult<()> {
    file.persist(path)
        .map(|_| ())
        .map_err(|error| io_error("install prepared analysis", path, error.error))
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

fn digest_file(path: &Path) -> SyncFixtureResult<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(FILE_DIGEST_MAGIC);
    hash_file_contents(path, &mut hasher)?;
    Ok(hasher.finalize().into())
}

fn digest_tree(root: &Path) -> SyncFixtureResult<[u8; 32]> {
    let root = canonicalize("canonicalize fixture tree", root)?;
    if !root.is_dir() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "fixture tree '{}' is not a directory",
            root.display(),
        )));
    }
    let mut entries = Vec::new();
    collect_tree_entries(&root, &root, &mut entries)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    if entries.is_empty() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "fixture tree '{}' has no files",
            root.display(),
        )));
    }

    let mut hasher = Sha256::new();
    hasher.update(TREE_DIGEST_MAGIC);
    hasher.update((entries.len() as u64).to_le_bytes());
    for entry in entries {
        hasher.update((entry.relative.len() as u64).to_le_bytes());
        hasher.update(entry.relative.as_bytes());
        hash_file_contents(&entry.path, &mut hasher)?;
    }
    Ok(hasher.finalize().into())
}

struct TreeEntry {
    path: PathBuf,
    relative: String,
}

fn collect_tree_entries(
    root: &Path,
    directory: &Path,
    output: &mut Vec<TreeEntry>,
) -> SyncFixtureResult<()> {
    let entries = read_dir("scan fixture tree", directory)?;
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| io_error("inspect fixture tree entry", &path, error))?;
        if kind.is_symlink() {
            return Err(SyncFixtureError::InvalidConfig(format!(
                "fixture tree '{}' contains symlink '{}'",
                root.display(),
                path.display(),
            )));
        }
        if kind.is_dir() {
            collect_tree_entries(root, &path, output)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| {
                SyncFixtureError::InvalidConfig(format!(
                    "fixture '{}' escaped root '{}'",
                    path.display(),
                    root.display(),
                ))
            })?;
            output.push(TreeEntry {
                relative: portable_relative_path(relative)?,
                path,
            });
        } else {
            return Err(SyncFixtureError::InvalidConfig(format!(
                "fixture tree entry '{}' is not a regular file or directory",
                path.display(),
            )));
        }
    }
    Ok(())
}

fn portable_relative_path(path: &Path) -> SyncFixtureResult<String> {
    let mut output = String::new();
    for component in path.components() {
        let component = component.as_os_str().to_str().ok_or_else(|| {
            SyncFixtureError::InvalidConfig(format!(
                "fixture path '{}' is not valid UTF-8",
                path.display(),
            ))
        })?;
        if !output.is_empty() {
            output.push('/');
        }
        output.push_str(component);
    }
    Ok(output)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified: SystemTime,
}

fn file_stamp(path: &Path) -> SyncFixtureResult<FileStamp> {
    let metadata =
        fs::metadata(path).map_err(|error| io_error("read file metadata", path, error))?;
    if !metadata.is_file() {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "'{}' is not a regular file",
            path.display(),
        )));
    }
    let modified = metadata
        .modified()
        .map_err(|error| io_error("read file modification time", path, error))?;
    Ok(FileStamp {
        len: metadata.len(),
        modified,
    })
}

fn hash_file_contents(path: &Path, hasher: &mut Sha256) -> SyncFixtureResult<()> {
    let before = file_stamp(path)?;
    hasher.update(before.len.to_le_bytes());
    let mut file =
        File::open(path).map_err(|error| io_error("open fixture content", path, error))?;
    let mut buffer = vec![0; HASH_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("read fixture content", path, error))?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            SyncFixtureError::InvalidConfig(format!("'{}' is too large", path.display()))
        })?;
        hasher.update(&buffer[..read]);
    }
    let after = file_stamp(path)?;
    if before != after || total != before.len {
        return Err(SyncFixtureError::InvalidConfig(format!(
            "'{}' changed while it was being hashed",
            path.display(),
        )));
    }
    Ok(())
}

fn collect_library_tracks(directory: &Path, output: &mut Vec<PathBuf>) -> SyncFixtureResult<()> {
    let entries = read_dir("scan sync music library", directory)?;
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| io_error("inspect library entry", &path, error))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_library_tracks(&path, output)?;
        } else if kind.is_file() && is_supported_audio(&path) {
            output.push(canonicalize("canonicalize library track", &path)?);
        }
    }
    Ok(())
}

fn read_dir(operation: &'static str, path: &Path) -> SyncFixtureResult<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| io_error(operation, path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io_error(operation, path, error))?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}

fn canonicalize(operation: &'static str, path: &Path) -> SyncFixtureResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| io_error(operation, path, error))
}

fn io_error(operation: &'static str, path: &Path, error: io::Error) -> SyncFixtureError {
    SyncFixtureError::Io {
        operation,
        path: path.to_path_buf(),
        error,
    }
}

async fn blocking<T, F>(operation: &'static str, work: F) -> SyncFixtureResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> SyncFixtureResult<T> + Send + 'static,
{
    task::spawn_blocking(work)
        .await
        .map_err(|error| SyncFixtureError::Blocking {
            operation,
            detail: error.to_string(),
        })?
}

fn stable_index(seed: u64, len: usize) -> usize {
    (mix_seed(seed) % len as u64) as usize
}

fn mix_seed(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use kithara::audio::{Bucket, GridSegment};

    use super::*;

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

    #[test]
    fn seeded_library_selection_is_stable_and_content_distinct() {
        let root = tempfile::tempdir().expect("library temp dir");
        fs::write(root.path().join("a.mp3"), b"same").expect("write a");
        fs::write(root.path().join("b.flac"), b"same").expect("write b");
        fs::write(root.path().join("c.wav"), b"different").expect("write c");
        fs::write(root.path().join("cover.jpg"), b"ignored").expect("write cover");

        let first = select_library_pair(root.path(), 42).expect("select library pair");
        let second = select_library_pair(root.path(), 42).expect("repeat library selection");

        assert_eq!(first.deck_a().path(), second.deck_a().path());
        assert_eq!(first.deck_b().path(), second.deck_b().path());
        assert_ne!(first.deck_a().content_digest, first.deck_b().content_digest);
        assert_eq!(first.library_seed(), Some(42));
    }

    #[test]
    fn library_with_only_duplicate_audio_is_rejected() {
        let root = tempfile::tempdir().expect("library temp dir");
        fs::write(root.path().join("a.mp3"), b"same").expect("write a");
        fs::write(root.path().join("b.flac"), b"same").expect("write b");

        let error = select_library_pair(root.path(), 7).expect_err("duplicates must fail");
        assert!(error.to_string().contains("distinct content"));
    }

    #[test]
    fn absent_library_is_no_case_and_half_explicit_pair_is_invalid() {
        let absent = LibraryEnv {
            root: None,
            seed: None,
            track_a: Some(OsString::from("ignored.mp3")),
            track_b: None,
        };
        assert!(
            library_pair_from_values(absent)
                .expect("absent opt-in is not an error")
                .is_none()
        );

        let root = tempfile::tempdir().expect("library temp dir");
        let half = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: None,
            track_a: Some(OsString::from("one.mp3")),
            track_b: None,
        };
        let error = library_pair_from_values(half).expect_err("half pair must fail");
        assert!(error.to_string().contains("must either both be set"));
    }

    #[test]
    fn explicit_tracks_must_stay_inside_the_opted_in_library() {
        let root = tempfile::tempdir().expect("library temp dir");
        let outside = tempfile::tempdir().expect("outside temp dir");
        fs::write(root.path().join("inside.mp3"), b"inside").expect("write inside");
        fs::write(outside.path().join("outside.mp3"), b"outside").expect("write outside");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: None,
            track_a: Some(OsString::from("inside.mp3")),
            track_b: Some(outside.path().join("outside.mp3").into_os_string()),
        };

        let error = library_pair_from_values(values).expect_err("outside track must fail");
        assert!(error.to_string().contains("is outside"));
    }

    #[test]
    fn malformed_seed_is_rejected_even_with_an_explicit_pair() {
        let root = tempfile::tempdir().expect("library temp dir");
        fs::write(root.path().join("a.mp3"), b"a").expect("write a");
        fs::write(root.path().join("b.mp3"), b"b").expect("write b");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: Some(OsString::from("not-a-number")),
            track_a: Some(OsString::from("a.mp3")),
            track_b: Some(OsString::from("b.mp3")),
        };

        let error = library_pair_from_values(values).expect_err("invalid seed must fail");
        assert!(error.to_string().contains("unsigned 64-bit integer"));
    }

    #[test]
    fn explicit_pair_preserves_the_replay_seed() {
        let root = tempfile::tempdir().expect("library temp dir");
        fs::write(root.path().join("a.mp3"), b"a").expect("write a");
        fs::write(root.path().join("b.mp3"), b"b").expect("write b");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: Some(OsString::from("99")),
            track_a: Some(OsString::from("a.mp3")),
            track_b: Some(OsString::from("b.mp3")),
        };

        let pair = library_pair_from_values(values)
            .expect("explicit pair resolves")
            .expect("library opt-in yields a pair");

        assert_eq!(pair.library_seed(), Some(99));
    }

    #[test]
    fn file_digest_is_content_addressed_not_path_addressed() {
        let root = tempfile::tempdir().expect("digest temp dir");
        let a = root.path().join("a.mp3");
        let b = root.path().join("b.mp3");
        fs::write(&a, b"identical bytes").expect("write a");
        fs::write(&b, b"identical bytes").expect("write b");

        assert_eq!(
            digest_file(&a).expect("digest a"),
            digest_file(&b).expect("digest b")
        );
    }

    #[test]
    fn tree_digest_covers_relative_names_and_file_bytes() {
        let root = tempfile::tempdir().expect("tree temp dir");
        fs::write(root.path().join("a.bin"), b"bytes").expect("write a");
        let first = digest_tree(root.path()).expect("first digest");
        fs::rename(root.path().join("a.bin"), root.path().join("b.bin")).expect("rename");
        let renamed = digest_tree(root.path()).expect("renamed digest");
        fs::write(root.path().join("b.bin"), b"changed").expect("change bytes");
        let changed = digest_tree(root.path()).expect("changed digest");

        assert_ne!(first, renamed);
        assert_ne!(renamed, changed);
    }

    #[test]
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

    #[test]
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

    #[test]
    fn prepared_analysis_writer_replaces_one_exact_file() {
        let root = tempfile::tempdir().expect("prepared analysis temp dir");
        let path = root.path().join("fixture.ksan");

        write_prepared_file(&path, b"first").expect("write first fixture");
        write_prepared_file(&path, b"second").expect("replace fixture");

        assert_eq!(fs::read(path).expect("read replaced fixture"), b"second");
    }

    #[test]
    fn failed_prepared_analysis_install_preserves_the_current_file() {
        let root = tempfile::tempdir().expect("prepared analysis temp dir");
        let path = root.path().join("fixture.ksan");
        fs::write(&path, b"current").expect("write current fixture");
        let mut staged = NamedTempFile::new_in(root.path()).expect("create staged fixture");
        staged
            .write_all(b"replacement")
            .expect("write staged fixture");
        let displaced = root.path().join("displaced.tmp");
        fs::rename(staged.path(), &displaced).expect("make staged path unavailable");

        persist_prepared_file(staged, &path).expect_err("missing staged inode must fail install");

        assert_eq!(fs::read(path).expect("read current fixture"), b"current");
    }

    #[kithara::test(tokio)]
    async fn prepared_analysis_rejects_media_changed_after_identity_resolution() {
        let root = tempfile::tempdir().expect("prepared analysis temp dir");
        let path = root.path().join("fixture.mp3");
        fs::write(&path, b"first").expect("write initial media");
        let mut track = local_fixture(&path).expect("resolve initial media identity");
        track.prepared_analysis = Some(PreparedAnalysis::TestMp3);
        fs::write(&path, b"second").expect("replace media bytes");
        let fixtures = SyncAnalysisFixtures::production().expect("production analysis config");

        let error = fixtures
            .load_prepared(&track)
            .await
            .expect_err("changed media must fail before sidecar loading");

        assert!(error.to_string().contains("changed after"));
    }

    #[kithara::test(tokio)]
    async fn checked_in_test_mp3_has_current_prepared_analysis() {
        let path = repository_assets_root().join("test.mp3");
        let track = SyncTrackFixture {
            media: "repo:test.mp3".to_owned(),
            source: ResourceSrc::Path(path.clone()),
            content_source: ContentSource::File(path.clone()),
            content_digest: digest_file(&path).expect("digest checked-in MP3"),
            profile: AnalysisProfile::Progressive,
            prepared_analysis: Some(PreparedAnalysis::TestMp3),
        };
        let fixtures = SyncAnalysisFixtures::production().expect("production analysis config");

        let prepared = fixtures
            .load_prepared(&track)
            .await
            .expect("checked-in prepared analysis matches the current MP3 and analyzer");

        validate_complete_analysis(&prepared.analysis, track.media())
            .expect("prepared analysis remains usable");
    }

    #[test]
    fn analysis_key_changes_with_content_profile_and_configuration() {
        let root = tempfile::tempdir().expect("key temp dir");
        let path = root.path().join("track.mp3");
        let other_path = root.path().join("other.mp3");
        fs::write(&path, b"track").expect("write track");
        fs::write(&other_path, b"other").expect("write other track");
        let progressive = local_fixture(&path).expect("progressive fixture");
        let other_content = local_fixture(&other_path).expect("other fixture");
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
        assert_ne!(
            prepared_analysis_key(&progressive, "config-a"),
            prepared_analysis_key(&progressive, "config-b"),
        );
        assert_ne!(
            prepared_analysis_key(&progressive, "config-a"),
            prepared_analysis_key(&other_content, "config-a"),
        );
        assert_ne!(
            prepared_analysis_key(&progressive, "config-a"),
            prepared_analysis_key(&hls, "config-a"),
        );
    }
}
