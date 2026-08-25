use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

use kithara::{
    assets::{AssetsError, ReadSide, ResourceKey},
    platform::time::SystemTime,
    play::ResourceSrc,
};
use sha2::{Digest, Sha256};

use super::{SyncFixtureError, SyncFixtureResources, SyncFixtureResult, blocking};
use crate::TestServerHelper;

const LIBRARY_ENV: &str = "KITHARA_SYNC_LIBRARY";
const LIBRARY_SEED_ENV: &str = "KITHARA_SYNC_LIBRARY_SEED";
const LIBRARY_TRACK_A_ENV: &str = "KITHARA_SYNC_LIBRARY_TRACK_A";
const LIBRARY_TRACK_B_ENV: &str = "KITHARA_SYNC_LIBRARY_TRACK_B";

const DEFAULT_LIBRARY_SEED: u64 = 0x4b49_5448_4152_4101;
const FILE_DIGEST_MAGIC: &[u8] = b"kithara-sync-file\0";
const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const HLS_ANALYSIS_VARIANT: u32 = 0;
const TREE_DIGEST_MAGIC: &[u8] = b"kithara-sync-tree\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryMp3 {
    Test,
    Track,
}

impl RepositoryMp3 {
    const fn asset_name(self) -> &'static str {
        match self {
            Self::Test => "test.mp3",
            Self::Track => "track.mp3",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AnalysisProfile {
    Progressive,
    Hls { variant: u32 },
}

impl AnalysisProfile {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::Progressive => 0,
            Self::Hls { .. } => 1,
        }
    }

    pub(super) const fn variant(self) -> u32 {
        match self {
            Self::Progressive => u32::MAX,
            Self::Hls { variant } => variant,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum ContentSource {
    File(PathBuf),
    Tree(PathBuf),
}

impl ContentSource {
    pub(super) fn digest(&self, resources: &SyncFixtureResources) -> SyncFixtureResult<[u8; 32]> {
        match self {
            Self::File(path) => digest_file(resources, path),
            Self::Tree(path) => digest_tree(resources, path),
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SyncTrackFixture {
    pub(super) media: String,
    pub(super) source: ResourceSrc,
    pub(super) content_source: ContentSource,
    pub(super) content_digest: [u8; 32],
    pub(super) profile: AnalysisProfile,
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

pub async fn library_pair_from_env(
    resources: &SyncFixtureResources,
) -> SyncFixtureResult<Option<SyncFixturePair>> {
    let values = LibraryEnv::read();
    if values.root.is_none() {
        return Ok(None);
    }
    let resources = resources.clone();
    blocking("resolve opt-in sync library", move || {
        library_pair_from_values(&resources, values)
    })
    .await
}

fn select_library_pair(
    resources: &SyncFixtureResources,
    root: &Path,
    seed: u64,
) -> SyncFixtureResult<SyncFixturePair> {
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
    let deck_a = local_fixture(resources, &tracks[first_index])?;
    let second_start = stable_index(mix_seed(seed), tracks.len() - 1);
    for offset in 0..tracks.len() - 1 {
        let slot = (second_start + offset) % (tracks.len() - 1);
        let second_index = if slot >= first_index { slot + 1 } else { slot };
        let deck_b = local_fixture(resources, &tracks[second_index])?;
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
    resources: &SyncFixtureResources,
    server: &TestServerHelper,
    which: RepositoryMp3,
) -> SyncFixtureResult<SyncTrackFixture> {
    let asset_name = which.asset_name();
    let path = repository_assets_root().join(asset_name);
    let content_path = path.clone();
    let resources = resources.clone();
    let digest = blocking("hash repository MP3", move || {
        digest_file(&resources, &content_path)
    })
    .await?;
    Ok(SyncTrackFixture {
        media: format!("repo:{asset_name}"),
        source: ResourceSrc::Url(server.asset(asset_name)),
        content_source: ContentSource::File(path),
        content_digest: digest,
        profile: AnalysisProfile::Progressive,
    })
}

pub async fn repository_mp3_pair(
    resources: &SyncFixtureResources,
    server: &TestServerHelper,
) -> SyncFixtureResult<SyncFixturePair> {
    let deck_a = repository_mp3(resources, server, RepositoryMp3::Test).await?;
    let deck_b = repository_mp3(resources, server, RepositoryMp3::Track).await?;
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

pub async fn silvercomet_hls(
    resources: &SyncFixtureResources,
    server: &TestServerHelper,
) -> SyncFixtureResult<SyncTrackFixture> {
    let path = repository_assets_root().join("hls");
    let content_path = path.clone();
    let resources = resources.clone();
    let digest = blocking("hash local Silvercomet HLS", move || {
        digest_tree(&resources, &content_path)
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
    })
}

fn library_pair_from_values(
    resources: &SyncFixtureResources,
    values: LibraryEnv,
) -> SyncFixtureResult<Option<SyncFixturePair>> {
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
            let deck_a = local_fixture(resources, &track_a)?;
            let deck_b = local_fixture(resources, &track_b)?;
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
        (None, None) => select_library_pair(resources, &root, seed).map(Some),
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

pub(super) fn local_fixture(
    resources: &SyncFixtureResources,
    path: &Path,
) -> SyncFixtureResult<SyncTrackFixture> {
    let canonical = canonicalize("canonicalize local sync track", path)?;
    validate_audio_file(&canonical, "sync track")?;
    let digest = digest_file(resources, &canonical)?;
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

fn digest_file(resources: &SyncFixtureResources, path: &Path) -> SyncFixtureResult<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(FILE_DIGEST_MAGIC);
    hash_file_contents(resources, path, &mut hasher)?;
    Ok(hasher.finalize().into())
}

fn digest_tree(resources: &SyncFixtureResources, root: &Path) -> SyncFixtureResult<[u8; 32]> {
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
        hash_file_contents(resources, &entry.path, &mut hasher)?;
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

fn hash_file_contents(
    resources: &SyncFixtureResources,
    path: &Path,
    hasher: &mut Sha256,
) -> SyncFixtureResult<()> {
    let before = file_stamp(path)?;
    hasher.update(before.len.to_le_bytes());
    let key = ResourceKey::absolute(path).map_err(|error| SyncFixtureError::Asset {
        operation: "key fixture content",
        path: path.to_path_buf(),
        error: Box::new(error),
    })?;
    let reader = resources
        .store()
        .open_resource(&key, None)
        .map_err(|error| SyncFixtureError::Asset {
            operation: "open fixture content",
            path: path.to_path_buf(),
            error: Box::new(error),
        })?;
    let mut buffer = resources.byte_pool().get();
    buffer
        .ensure_len(HASH_BUFFER_BYTES)
        .map_err(|error| SyncFixtureError::InvalidConfig(error.to_string()))?;
    let mut total = 0_u64;
    loop {
        let read = reader
            .read_at(total, &mut buffer)
            .map_err(AssetsError::from)
            .map_err(|error| SyncFixtureError::Asset {
                operation: "read fixture content",
                path: path.to_path_buf(),
                error: Box::new(error),
            })?;
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

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs};

    use kithara_test_utils::kithara;

    use super::*;
    use crate::sync_fixture::test_resources;

    #[kithara::test]
    fn seeded_library_selection_is_stable_and_content_distinct() {
        let root = tempfile::tempdir().expect("library temp dir");
        let resources = test_resources("sync-fixture-unit-seeded-library");
        fs::write(root.path().join("a.mp3"), b"same").expect("write a");
        fs::write(root.path().join("b.flac"), b"same").expect("write b");
        fs::write(root.path().join("c.wav"), b"different").expect("write c");
        fs::write(root.path().join("cover.jpg"), b"ignored").expect("write cover");

        let first = select_library_pair(&resources, root.path(), 42).expect("select library pair");
        let second =
            select_library_pair(&resources, root.path(), 42).expect("repeat library selection");

        assert_eq!(first.deck_a().path(), second.deck_a().path());
        assert_eq!(first.deck_b().path(), second.deck_b().path());
        assert_ne!(first.deck_a().content_digest, first.deck_b().content_digest);
        assert_eq!(first.library_seed(), Some(42));
    }

    #[kithara::test]
    fn library_with_only_duplicate_audio_is_rejected() {
        let root = tempfile::tempdir().expect("library temp dir");
        let resources = test_resources("sync-fixture-unit-duplicate-library");
        fs::write(root.path().join("a.mp3"), b"same").expect("write a");
        fs::write(root.path().join("b.flac"), b"same").expect("write b");

        let error =
            select_library_pair(&resources, root.path(), 7).expect_err("duplicates must fail");
        assert!(error.to_string().contains("distinct content"));
    }

    #[kithara::test]
    fn absent_library_is_no_case_and_half_explicit_pair_is_invalid() {
        let resources = test_resources("sync-fixture-unit-absent-library");
        let absent = LibraryEnv {
            root: None,
            seed: None,
            track_a: Some(OsString::from("ignored.mp3")),
            track_b: None,
        };
        assert!(
            library_pair_from_values(&resources, absent)
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
        let error = library_pair_from_values(&resources, half).expect_err("half pair must fail");
        assert!(error.to_string().contains("must either both be set"));
    }

    #[kithara::test]
    fn explicit_tracks_must_stay_inside_the_opted_in_library() {
        let root = tempfile::tempdir().expect("library temp dir");
        let outside = tempfile::tempdir().expect("outside temp dir");
        let resources = test_resources("sync-fixture-unit-outside-library");
        fs::write(root.path().join("inside.mp3"), b"inside").expect("write inside");
        fs::write(outside.path().join("outside.mp3"), b"outside").expect("write outside");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: None,
            track_a: Some(OsString::from("inside.mp3")),
            track_b: Some(outside.path().join("outside.mp3").into_os_string()),
        };

        let error =
            library_pair_from_values(&resources, values).expect_err("outside track must fail");
        assert!(error.to_string().contains("is outside"));
    }

    #[kithara::test]
    fn malformed_seed_is_rejected_even_with_an_explicit_pair() {
        let root = tempfile::tempdir().expect("library temp dir");
        let resources = test_resources("sync-fixture-unit-malformed-seed");
        fs::write(root.path().join("a.mp3"), b"a").expect("write a");
        fs::write(root.path().join("b.mp3"), b"b").expect("write b");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: Some(OsString::from("not-a-number")),
            track_a: Some(OsString::from("a.mp3")),
            track_b: Some(OsString::from("b.mp3")),
        };

        let error =
            library_pair_from_values(&resources, values).expect_err("invalid seed must fail");
        assert!(error.to_string().contains("unsigned 64-bit integer"));
    }

    #[kithara::test]
    fn explicit_pair_preserves_the_replay_seed() {
        let root = tempfile::tempdir().expect("library temp dir");
        let resources = test_resources("sync-fixture-unit-explicit-seed");
        fs::write(root.path().join("a.mp3"), b"a").expect("write a");
        fs::write(root.path().join("b.mp3"), b"b").expect("write b");
        let values = LibraryEnv {
            root: Some(root.path().as_os_str().to_owned()),
            seed: Some(OsString::from("99")),
            track_a: Some(OsString::from("a.mp3")),
            track_b: Some(OsString::from("b.mp3")),
        };

        let pair = library_pair_from_values(&resources, values)
            .expect("explicit pair resolves")
            .expect("library opt-in yields a pair");

        assert_eq!(pair.library_seed(), Some(99));
    }

    #[kithara::test]
    fn file_digest_is_content_addressed_not_path_addressed() {
        let root = tempfile::tempdir().expect("digest temp dir");
        let resources = test_resources("sync-fixture-unit-file-digest");
        let a = root.path().join("a.mp3");
        let b = root.path().join("b.mp3");
        fs::write(&a, b"identical bytes").expect("write a");
        fs::write(&b, b"identical bytes").expect("write b");

        assert_eq!(
            digest_file(&resources, &a).expect("digest a"),
            digest_file(&resources, &b).expect("digest b")
        );
    }

    #[kithara::test]
    fn tree_digest_covers_relative_names_and_file_bytes() {
        let root = tempfile::tempdir().expect("tree temp dir");
        let resources = test_resources("sync-fixture-unit-tree-digest");
        fs::write(root.path().join("a.bin"), b"bytes").expect("write a");
        let first = digest_tree(&resources, root.path()).expect("first digest");
        fs::rename(root.path().join("a.bin"), root.path().join("b.bin")).expect("rename");
        let renamed = digest_tree(&resources, root.path()).expect("renamed digest");
        fs::write(root.path().join("b.bin"), b"changed").expect("change bytes");
        let changed = digest_tree(&resources, root.path()).expect("changed digest");

        assert_ne!(first, renamed);
        assert_ne!(renamed, changed);
    }
}
