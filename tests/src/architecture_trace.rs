use std::path::Path;

use anyhow::Result;
use kithara_devtools::viz::trace::{TraceRecord, TraceRecordKind, TraceSource, TraceWriter};

use crate::usdt_trace::Scope;

/// Writes `records`, then every probe firing `probes` recorded, in firing order.
pub fn write(
    path: &Path,
    records: impl IntoIterator<Item = TraceRecord>,
    probes: &Scope,
) -> Result<()> {
    let mut writer = TraceWriter::create(path)?;
    for record in records {
        writer.write(&record)?;
    }
    for (index, event) in probes.events().into_iter().enumerate() {
        let sequence = 10_000 + index as u64;
        let mut record = TraceRecord::new(sequence, TraceRecordKind::Event, event.probe);
        if let (Some(path), Some(line)) = (event.file, event.line) {
            record =
                record.with_source(TraceSource::new(workspace_relative(path), line as usize, 0));
        }
        writer.write(&record.with_thread(format!("{:?}", event.thread)))?;
    }
    writer.finish()
}

/// Trims a compile-time source path to the workspace-relative form the
/// architecture graph keys its nodes by.
fn workspace_relative(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    for marker in ["/crates/", "/tests/", "/xtask/"] {
        if let Some((_, suffix)) = normalized.split_once(marker) {
            return format!("{}{suffix}", marker.trim_start_matches('/'));
        }
    }
    normalized
}
