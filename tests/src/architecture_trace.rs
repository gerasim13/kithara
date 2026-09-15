use std::path::Path;

use anyhow::Result;
use kithara_devtools::viz::trace::{TraceRecord, TraceRecordKind, TraceWriter};

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
        writer.write(&TraceRecord::new(
            sequence,
            TraceRecordKind::Event,
            format!("{}::{}", event.target, event.probe),
        ))?;
    }
    writer.finish()
}
