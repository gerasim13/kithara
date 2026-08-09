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

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
};

use toml::{Table, Value};

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

/// The crates whose resolved build determines an encoded byte. Read from the
/// lockfile by name, so a version bump to any of them still lands in a fresh
/// namespace.
const ENCODERS: &[&str] = &["fdk-aac", "fdk-aac-sys", "ffmpeg-next", "ffmpeg-sys-next"];

const LOCKFILE: &str = "../Cargo.lock";

/// Hash what the lockfile resolved for [`ENCODERS`], rather than the lockfile.
///
/// The whole file was hashed here before, on the reasoning that a dependency
/// version determines the encoded bytes. Only these ones do: bumping an XML
/// parser cannot change an AAC frame, but hashing the file said it could, so
/// every unrelated dependency bump moved the namespace and left the suite with
/// an empty cache — and a cold cache means per-test ffmpeg re-encodes, which
/// blow the budgets of tests that assert against a deadline. That is the same
/// failure the `src/native` narrowing above was written to stop; the lockfile
/// was the remaining wide input.
fn hash_encoder_versions(hasher: &mut DefaultHasher) {
    println!("cargo:rerun-if-changed={LOCKFILE}");
    let text = fs::read_to_string(LOCKFILE).expect("the workspace lockfile must be readable");
    let lock: Table = text.parse().expect("the workspace lockfile must be TOML");
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .expect("the workspace lockfile must list packages");

    let mut resolved: Vec<String> = packages
        .iter()
        .filter_map(|package| {
            let name = package.get("name").and_then(Value::as_str)?;
            ENCODERS.contains(&name).then(|| {
                let field = |key| package.get(key).and_then(Value::as_str).unwrap_or_default();
                // The checksum pins the exact bytes the version resolves to,
                // which a re-release under one version number would not.
                format!("{name} {} {}", field("version"), field("checksum"))
            })
        })
        .collect();
    resolved.sort();

    // A crate leaving the lock under a name listed here would silently stop
    // being tracked, and an untracked encoder change is a cache serving bytes
    // it should not. Fail the build instead.
    assert_eq!(
        resolved.len(),
        ENCODERS.len(),
        "{LOCKFILE} resolved {resolved:?} for {ENCODERS:?}; update ENCODERS to \
         match the crates that encode fixtures"
    );
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
    ] {
        if let Ok(bytes) = fs::read(file) {
            bytes.hash(&mut hasher);
        }
        println!("cargo:rerun-if-changed={file}");
    }
    hash_encoder_versions(&mut hasher);
    println!("cargo:rerun-if-changed=build.rs");

    println!(
        "cargo:rustc-env=KITHARA_FIXTURE_BUILD={:016x}",
        hasher.finish()
    );
}
