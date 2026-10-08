use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{self, Command},
};

use sha2::{Digest, Sha256};

/// Where a fetched model lands, so a rebuild does not fetch it again.
const CACHE_ENV: &str = "KITHARA_BEAT_MODEL_CACHE";
const FULL_FILE: &str = "beat_this_full.onnx";

/// A model the build names to the compiler through `env`. `source` is the URL
/// it is fetched from and the SHA-256 it has to hash to; a model without one
/// has to be in the cache already.
struct Model {
    env: &'static str,
    file: &'static str,
    source: Option<(&'static str, &'static str)>,
}

fn main() {
    const MEL: Model = Model {
        env: "KITHARA_MEL_MODEL",
        file: "mel_spectrogram.onnx",
        source: Some((
            "https://raw.githubusercontent.com/danigb/beat-this-rs/089b509247e6fdcec666511c0dcf0d5f39c21e73/models/mel_spectrogram.onnx",
            "fdd59e65c515331308e4c8841edf99972deca646bdf6197744c2a5b7755e3de9",
        )),
    };
    const SMALL: Model = Model {
        env: "KITHARA_BEAT_MODEL",
        file: "beat_this_small.onnx",
        source: Some((
            "https://raw.githubusercontent.com/danigb/beat-this-rs/089b509247e6fdcec666511c0dcf0d5f39c21e73/models/beat_this_small.onnx",
            "a5f8d39d989f31859454ba27afe61c5317ca95e4d9373e6853e5361b8937172f",
        )),
    };
    const FULL: Model = Model {
        env: "KITHARA_BEAT_MODEL",
        file: FULL_FILE,
        source: Some((
            "https://github.com/danigb/beat-this-rs/releases/download/model-large/beat_this.onnx",
            "5f810debe53459b559127fb55bbad40035bb47cc567b20e501670f968c770f02",
        )),
    };
    const INT8: Model = Model {
        env: "KITHARA_BEAT_MODEL",
        file: "beat_this_full_int8.onnx",
        source: None,
    };

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed={CACHE_ENV}");
    if env::var_os("CARGO_FEATURE_EMBED_MODEL").is_none() {
        return;
    }
    let cache = cache_dir();
    resolve(&cache, &MEL);
    for (feature, model) in [
        ("CARGO_FEATURE_EMBED_SMALL_MODEL", &SMALL),
        ("CARGO_FEATURE_EMBED_FULL_MODEL", &FULL),
        ("CARGO_FEATURE_EMBED_FULL_INT8_MODEL", &INT8),
    ] {
        if env::var_os(feature).is_some() {
            resolve(&cache, model);
        }
    }
}

fn cache_dir() -> PathBuf {
    if let Some(cache) = env::var_os(CACHE_ENV) {
        return PathBuf::from(cache);
    }
    println!("cargo::rerun-if-env-changed=CARGO_HOME");
    if let Some(cargo_home) = env::var_os("CARGO_HOME") {
        return PathBuf::from(cargo_home).join("kithara-beat-models");
    }
    println!("cargo::rerun-if-env-changed=HOME");
    let home = env::var_os("HOME").or_else(|| {
        println!("cargo::rerun-if-env-changed=USERPROFILE");
        env::var_os("USERPROFILE")
    });
    let Some(home) = home else {
        println!("cargo::error=cannot determine Cargo home; set {CACHE_ENV} or CARGO_HOME");
        process::exit(1);
    };
    PathBuf::from(home).join(".cargo/kithara-beat-models")
}

/// Names a build-owned snapshot to the compiler after resolving its source.
/// Upstream bytes must match their pinned digest; locally quantized bytes have
/// to be supplied by the user.
fn resolve(cache: &Path, model: &Model) {
    let file = model.file;
    let path = cache.join(file);
    let ready = if let Some((url, sha256)) = model.source {
        fetch(cache, &path, url, sha256)
    } else {
        println!("cargo::rerun-if-changed={}", path.display());
        let Some(_lock) = model_lock(cache, &path) else {
            return;
        };
        if !path.exists() {
            println!(
                "cargo::error={file} is missing from {}; quantize it with \
                 `uv run --with onnx --with onnxruntime \
                 https://raw.githubusercontent.com/danigb/beat-this-rs/main/scripts/quantize_int8.py \
                 --input {}/{FULL_FILE} --output {}`",
                cache.display(),
                cache.display(),
                path.display()
            );
            return;
        }
        let Some(bytes) = read_model(&path, None) else {
            return;
        };
        materialize(&path, &bytes)
    };
    if !ready {
        return;
    }
    let Some(output) = output_path(&path) else {
        return;
    };
    println!("cargo::rustc-env={}={}", model.env, output.display());
}

/// Holds the model name across source validation and output publication. CI
/// containers share the cache and can have colliding process ids.
fn model_lock(cache: &Path, path: &Path) -> Option<fs::File> {
    if let Err(err) = fs::create_dir_all(cache) {
        println!("cargo::error=cannot create {}: {err}", cache.display());
        return None;
    }
    let lock_path = path.with_extension("onnx.lock");
    let lock = match fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    {
        Ok(lock) => lock,
        Err(err) => {
            println!("cargo::error=cannot open {}: {err}", lock_path.display());
            return None;
        }
    };
    if let Err(err) = fs4::FileExt::lock(&lock) {
        println!("cargo::error=cannot lock {}: {err}", lock_path.display());
        return None;
    }
    Some(lock)
}

/// Validates a cached source or fetches a missing one, then publishes the
/// validated bytes while still holding the model-name lock. An interrupted
/// download leaves its partial file for the next holder to overwrite.
fn fetch(cache: &Path, path: &Path, url: &str, sha256: &str) -> bool {
    let Some(_lock) = model_lock(cache, path) else {
        return false;
    };
    if path.exists() {
        let Some(bytes) = read_model(path, Some(sha256)) else {
            return false;
        };
        return materialize(path, &bytes);
    }
    let partial = path.with_extension("onnx.part");
    let status = Command::new("curl")
        .args(["-fL", "--retry", "3", "-o"])
        .arg(&partial)
        .arg(url)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            println!("cargo::error=curl {url} exited with {status}");
            let _ = fs::remove_file(&partial);
            return false;
        }
        Err(err) => {
            println!("cargo::error=cannot run curl to fetch {url}: {err}");
            return false;
        }
    }
    let Some(bytes) = read_model(&partial, Some(sha256)) else {
        let _ = fs::remove_file(&partial);
        return false;
    };
    if let Err(err) = fs::rename(&partial, path) {
        println!("cargo::error=cannot place {}: {err}", path.display());
        return false;
    }
    materialize(path, &bytes)
}

fn read_model(path: &Path, expected: Option<&str>) -> Option<Vec<u8>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            println!("cargo::error=cannot read {}: {err}", path.display());
            return None;
        }
    };
    if let Some(expected) = expected {
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected {
            println!(
                "cargo::error={} hashes to {actual}, expected {expected}",
                path.display()
            );
            return None;
        }
    }
    Some(bytes)
}

fn output_path(source: &Path) -> Option<PathBuf> {
    let Some(out) = env::var_os("OUT_DIR") else {
        println!("cargo::error=OUT_DIR is not set");
        return None;
    };
    let Some(file) = source.file_name() else {
        println!("cargo::error={} has no model filename", source.display());
        return None;
    };
    Some(PathBuf::from(out).join(file))
}

/// Keeps an identical snapshot untouched so another resolution does not make
/// Cargo rebuild units that already embedded these bytes.
fn materialize(source: &Path, bytes: &[u8]) -> bool {
    let Some(output) = output_path(source) else {
        return false;
    };
    match fs::read(&output) {
        Ok(current) if current == bytes => return true,
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => {
            println!("cargo::error=cannot read {}: {err}", output.display());
            return false;
        }
    }
    let partial = output.with_extension("onnx.part");
    if let Err(err) = fs::write(&partial, bytes) {
        println!("cargo::error=cannot write {}: {err}", partial.display());
        return false;
    }
    if let Err(err) = fs::rename(&partial, &output) {
        println!("cargo::error=cannot place {}: {err}", output.display());
        return false;
    }
    true
}
