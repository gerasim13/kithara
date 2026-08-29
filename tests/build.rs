//! Emits `KITHARA_FIXTURE_BUILD`: a fingerprint of the fixture-encoding code and
//! the encoder versions the lockfile resolved, baked into every test binary of
//! this package.
//!
//! The L2 fixture cache (see `src/fixture_cache.rs`) namespaces its selected
//! cache root by this value, so:
//! - all test binaries of one build (`suite_stress`, `suite_heavy`, …) share the
//!   same cache dir — an AAC fixture encoded by one binary is reused by every
//!   other binary and by repeated runs of the same build;
//! - a change to the encoding code or to an encoder crate's resolved version
//!   yields a fresh namespace, so a stale cache can never serve outdated bytes.

// Transitional: `kithara-fixtures` owns the encoder-version fingerprint now.
// This build script and its L2 cache both disappear in the migration's stage 2.
#[path = "../crates/kithara-fixtures/src/encoder_crates.rs"]
mod encoder_crates;

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

use encoder_crates::Lockfile;

/// Hash every `.rs` file under `dir` (path + contents) and register each for
/// change-tracking so the fingerprint refreshes whenever encoding code changes.
fn hash_rs_tree(dir: &Path, hasher: &mut DefaultHasher) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            hash_rs_tree(&path, hasher);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if let Ok(bytes) = fs::read(&path) {
                path.to_string_lossy().hash(hasher);
                bytes.hash(hasher);
            }
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

const LOCKFILE: &str = "../Cargo.lock";

fn hash_encoder_versions(hasher: &mut DefaultHasher) {
    println!("cargo:rerun-if-changed={LOCKFILE}");
    let text = fs::read_to_string(LOCKFILE).expect("the workspace lockfile must be readable");
    let lockfile = Lockfile::parse(&text).unwrap_or_else(|error| panic!("{LOCKFILE}: {error}"));

    // The list this hashes is hand-written, so it is guarded from both sides:
    // `encoder_versions` fails when a listed crate stops resolving, and an
    // unclassified new dependency of `kithara-encode` fails here — its version
    // would otherwise never reach the fingerprint, leaving the cache free to
    // serve bytes the previous encoder produced.
    let unclassified = lockfile
        .unclassified_encode_dependencies()
        .unwrap_or_else(|error| panic!("{LOCKFILE}: {error}"));
    assert!(
        unclassified.is_empty(),
        "{LOCKFILE}: unclassified kithara-encode dependencies: {unclassified:?}; add each crate \
         to ENCODERS if it can change an encoded byte, otherwise to NON_ENCODING_DEPENDENCIES"
    );

    let resolved = lockfile
        .encoder_versions()
        .unwrap_or_else(|error| panic!("{LOCKFILE}: {error}"));
    resolved.hash(hasher);
}

fn main() {
    let mut hasher = DefaultHasher::new();

    // Hash ONLY the spec→bytes transformation code: the encoders, the fMP4
    // muxer, the packaged-variant encode glue, and the signal encode route.
    // Spec changes flow into per-entry cache KEYS, and server delivery code
    // (routes/behavior, throttling, playlists) never changes encoded bytes —
    // hashing all of `src/native` forced a cold cache (and per-test ffmpeg
    // re-encodes blowing test budgets) on every test-server edit.
    for dir in ["../crates/kithara-encode/src", "src/native/fmp4"] {
        println!("cargo:rerun-if-changed={dir}");
        hash_rs_tree(Path::new(dir), &mut hasher);
    }
    // The encode glue and the PCM signal generator also determine the bytes.
    for file in [
        "src/native/hls_stream.rs",
        "src/native/routes/signal.rs",
        "src/signal_pcm.rs",
        "src/wav.rs",
    ] {
        if let Ok(bytes) = fs::read(file) {
            bytes.hash(&mut hasher);
        }
        println!("cargo:rerun-if-changed={file}");
    }
    hash_encoder_versions(&mut hasher);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../crates/kithara-fixtures/src/encoder_crates.rs");

    println!(
        "cargo:rustc-env=KITHARA_FIXTURE_BUILD={:016x}",
        hasher.finish()
    );
}
