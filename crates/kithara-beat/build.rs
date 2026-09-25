use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
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
    env::var_os(CACHE_ENV).map_or_else(
        || env::temp_dir().join("kithara-beat-models"),
        PathBuf::from,
    )
}

/// Puts the model in the cache and names it to the compiler. A model upstream
/// publishes is fetched and checked; one that is quantized locally can only be
/// reported missing.
fn resolve(cache: &Path, model: &Model) {
    let file = model.file;
    let path = cache.join(file);
    println!("cargo::rerun-if-changed={}", path.display());
    if !path.exists() {
        let Some((url, sha256)) = model.source else {
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
        };
        if !fetch(cache, &path, url, sha256) {
            return;
        }
    }
    println!("cargo::rustc-env={}={}", model.env, path.display());
}

/// Fetches into a file only this process writes and moves it into place once
/// it checks out, so builds sharing one cache never read a partial download.
fn fetch(cache: &Path, path: &Path, url: &str, sha256: &str) -> bool {
    if let Err(err) = fs::create_dir_all(cache) {
        println!("cargo::error=cannot create {}: {err}", cache.display());
        return false;
    }
    let partial = path.with_extension(format!("onnx.{}.part", std::process::id()));
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
    if !verify(&partial, sha256) {
        let _ = fs::remove_file(&partial);
        return false;
    }
    if let Err(err) = fs::rename(&partial, path) {
        println!("cargo::error=cannot place {}: {err}", path.display());
        return false;
    }
    true
}

fn verify(path: &Path, expected: &str) -> bool {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            println!("cargo::error=cannot read {}: {err}", path.display());
            return false;
        }
    };
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        println!(
            "cargo::error={} hashes to {actual}, expected {expected}",
            path.display()
        );
        return false;
    }
    true
}
