mod output;

use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::fs::MetadataExt,
};

use ::kithara::{
    audio::ChunkOutcome,
    platform::{
        CancelToken,
        time::{self, Duration, Instant},
    },
    prelude::Resource,
};
use kithara_test_utils::kithara;
use serde_json::{Value, json};

use crate::{
    analysis::fixtures::{app_config, memory_store},
    pools::AppTrackSource,
    sources::build_source,
};

async fn decode_source(path: &str, config: &crate::config::AppConfig) -> Result<Value, String> {
    let AppTrackSource::Config(mut source) = build_source(path, config) else {
        return Err("source did not yield a resource configuration".to_owned());
    };
    let cancel = config.shutdown.child();
    source.set_cancel(cancel.child());
    let result = async {
        let mut resource = Resource::new(*source).await.map_err(|error| error.to_string())?;
        resource.preload().await.map_err(|error| error.to_string())?;
        let mut reader = resource;
        let mut samples = 0u64;
        let mut nonzero = 0u64;
        let mut peak = 0.0f32;
        let mut frames = 0u64;
        let mut last_progress = Instant::now();
        loop {
            match reader.next_chunk().map_err(|error| error.to_string())? {
                ChunkOutcome::Chunk(chunk) => {
                    for &value in chunk.samples.iter() {
                        if !value.is_finite() {
                            return Err("decoded nonfinite PCM".to_owned());
                        }
                        nonzero += u64::from(value != 0.0);
                        peak = peak.max(value.abs());
                    }
                    samples += chunk.samples.len() as u64;
                    frames += (chunk.samples.len() / usize::from(chunk.spec().channels)) as u64;
                    last_progress = Instant::now();
                }
                ChunkOutcome::Pending { .. } => {
                    if last_progress.elapsed() > Duration::from_secs(30) {
                        return Err("no decode progress for 30 seconds".to_owned());
                    }
                    time::sleep(Duration::from_millis(1)).await;
                }
                ChunkOutcome::Eof { position } => {
                    if frames == 0 {
                        return Err("natural EOF without decoded frames".to_owned());
                    }
                    let spec = reader.spec();
                    return Ok(json!({"frames": frames, "samples": samples, "nonzero": nonzero,
                        "peak": peak, "channels": spec.channels, "sample_rate": spec.sample_rate.get(),
                        "eof_seconds": position.as_secs_f64()}));
                }
            }
        }
    }.await;
    cancel.cancel();
    result
}

/// Opt-in, resumable decode census through the application's source configuration.
#[kithara::test(native, tokio, flash(false))]
#[ignore = "requires the external music inventory and an explicit output ledger"]
async fn local_library_application_source_census() {
    census(false).await;
}

/// Opt-in full playback through the product offline Host with bounded output.
#[kithara::test(native, tokio, flash(false))]
#[ignore = "requires the external music inventory and an explicit output ledger"]
async fn local_library_application_output_census() {
    census(true).await;
}

async fn census(playback: bool) {
    let inventory = std::env::var("KITHARA_LIBRARY_INVENTORY").expect("inventory JSONL path");
    let ledger = std::env::var("KITHARA_LIBRARY_LEDGER").expect("output JSONL path");
    let filter = std::env::var("KITHARA_LIBRARY_FILTER").unwrap_or_default();
    let build = std::env::var("KITHARA_LIBRARY_BUILD").expect("source and feature fingerprint");
    let cancel = CancelToken::root();
    let config = app_config(&cancel, memory_store());
    let mut completed = std::collections::HashSet::new();
    if let Ok(previous) = File::open(&ledger) {
        for line in BufReader::new(previous).lines() {
            let row: Value =
                serde_json::from_str(&line.expect("ledger line")).expect("ledger JSON");
            if row["error"].is_null() {
                completed.insert(row["identity"].to_string());
            } else {
                completed.remove(&row["identity"].to_string());
            }
        }
    }
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ledger)
        .expect("open ledger");
    let input = BufReader::new(File::open(inventory).expect("open inventory"));
    let mut selected = 0;
    let mut checked = 0;
    let mut failed = 0;
    for line in input.lines() {
        let row: Value =
            serde_json::from_str(&line.expect("inventory line")).expect("inventory JSON");
        let path = row["path"].as_str().expect("inventory path");
        if !path.contains(&filter) {
            continue;
        }
        selected += 1;
        let metadata = std::fs::metadata(path).expect("source still exists");
        let identity = json!({"build": build, "playback": playback, "path": path, "bytes": metadata.len(), "device": metadata.dev(),
            "inode": metadata.ino(), "mtime": metadata.mtime(), "mtime_nsec": metadata.mtime_nsec()});
        if completed.contains(&identity.to_string()) {
            continue;
        }
        let result = time::timeout(Duration::from_secs(600), async {
            if playback {
                output::play_source(path, &config).await
            } else {
                decode_source(path, &config).await
            }
        })
        .await;
        let (pcm, error) = match result {
            Ok(Ok(pcm)) => (pcm, None),
            Ok(Err(error)) => (Value::Null, Some(error)),
            Err(error) => (Value::Null, Some(error.to_string())),
        };
        failed += usize::from(error.is_some());
        let entry = json!({"identity": identity, "pcm": pcm, "error": error});
        writeln!(output, "{entry}").expect("write result");
        output.flush().expect("flush result");
        checked += 1;
        if checked % 100 == 0 || entry["error"].is_string() {
            tracing::info!(checked, failed, path, error = ?entry["error"], "library decode census");
        }
    }
    cancel.cancel();
    assert!(selected > 0, "selection must contain audio");
    assert_eq!(
        failed, 0,
        "application-source failures are recorded in the ledger"
    );
}
