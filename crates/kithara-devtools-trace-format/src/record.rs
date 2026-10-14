use std::{
    fs::File,
    io::{BufWriter, Write as _},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const TRACE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceRecordKind {
    SpanEnter,
    SpanExit,
    Event,
    TaskSpawn,
    ResourceCreate,
    ResourceClone,
    ResourceTransfer,
    ResourceDrop,
    Send,
    Receive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(opt_in, get(deref = false, option_borrow_inner = false))]
pub struct TraceSource {
    #[field(get)]
    path: String,
    #[field(get, copy)]
    column: usize,
    #[field(get, copy)]
    line: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, fieldwork::Fieldwork)]
#[non_exhaustive]
#[serde(deny_unknown_fields)]
#[fieldwork(opt_in, get(deref = false, option_borrow_inner = false))]
pub struct TraceRecord {
    #[serde(default)]
    #[field(get)]
    correlation_id: Option<String>,
    #[serde(default)]
    #[field(get)]
    parent_span_id: Option<String>,
    #[serde(default)]
    #[field(get)]
    resource_id: Option<String>,
    #[serde(default)]
    #[field(get)]
    resource_type: Option<String>,
    #[serde(default)]
    #[field(get)]
    source: Option<TraceSource>,
    #[serde(default)]
    #[field(get)]
    span_id: Option<String>,
    #[serde(default)]
    #[field(get)]
    task_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[field(get)]
    name: String,
    #[field(get, copy)]
    kind: TraceRecordKind,
    #[field(get, copy)]
    schema_version: u32,
    #[field(get, copy)]
    sequence: u64,
}

pub struct TraceWriter {
    writer: BufWriter<File>,
}

impl TraceWriter {
    /// Creates a versioned JSONL trace at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the trace file cannot be created.
    pub fn create(path: &Path) -> Result<Self> {
        let file =
            File::create(path).with_context(|| format!("create trace: {}", path.display()))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    /// Flushes the trace to disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying file cannot be flushed.
    pub fn finish(mut self) -> Result<()> {
        self.writer.flush().context("flush architecture trace")
    }

    /// Appends one neutral trace record.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or writing fails.
    pub fn write(&mut self, record: &TraceRecord) -> Result<()> {
        serde_json::to_writer(&mut self.writer, record)?;
        self.writer.write_all(b"\n")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kithara_test_utils::kithara;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[kithara::test(native, flash(false))]
    fn writer_preserves_records_as_versioned_json_lines() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("trace.jsonl");
        let records: Vec<TraceRecord> = serde_json::from_value(json!([
            {
                "schema_version": TRACE_SCHEMA_VERSION, "sequence": 1,
                "kind": TraceRecordKind::Send, "name": "send",
                "source": { "path": "src/lib.rs", "line": 4, "column": 2 },
                "span_id": "producer", "parent_span_id": "root", "task_id": "worker",
                "thread_id": "thread-1", "correlation_id": "message-1",
                "resource_type": "Buffer", "resource_id": "7"
            },
            {
                "schema_version": TRACE_SCHEMA_VERSION, "sequence": 2,
                "kind": TraceRecordKind::Receive, "name": "receive",
                "correlation_id": "message-1"
            }
        ]))
        .expect("schema fixtures");
        let mut writer = TraceWriter::create(&path).expect("writer");
        for record in &records {
            writer.write(record).expect("record");
        }
        writer.finish().expect("finish");
        let jsonl = fs::read_to_string(&path).expect("read trace");
        assert_eq!(
            jsonl.lines().next(),
            Some(concat!(
                r#"{"correlation_id":"message-1","parent_span_id":"root","resource_id":"7","#,
                r#""resource_type":"Buffer","source":{"path":"src/lib.rs","column":2,"line":4},"#,
                r#""span_id":"producer","task_id":"worker","thread_id":"thread-1","#,
                r#""name":"send","kind":"send","schema_version":1,"sequence":1}"#,
            )),
        );
        let decoded = jsonl
            .lines()
            .map(|line| serde_json::from_str::<TraceRecord>(line).expect("trace record"))
            .collect::<Vec<_>>();

        assert!(jsonl.ends_with('\n'));
        assert_eq!(decoded, records);
        assert!(
            decoded
                .iter()
                .all(|record| record.schema_version == TRACE_SCHEMA_VERSION)
        );
    }

    #[kithara::test(native, flash(false))]
    fn records_default_optional_fields_and_reject_unknown_fields() {
        let minimal = r#"{"name":"send","kind":"send","schema_version":1,"sequence":1}"#;
        let record: TraceRecord = serde_json::from_str(minimal).expect("minimal record");
        assert_eq!(record.kind(), TraceRecordKind::Send);
        assert_eq!(record.name(), "send");
        assert_eq!(record.schema_version(), TRACE_SCHEMA_VERSION);
        assert_eq!(record.sequence(), 1);
        assert!(record.correlation_id.is_none());
        assert!(record.parent_span_id.is_none());
        assert!(record.resource_id.is_none());
        assert!(record.resource_type.is_none());
        assert!(record.source.is_none());
        assert!(record.span_id.is_none());
        assert!(record.task_id.is_none());
        assert!(record.thread_id.is_none());
        let unknown =
            r#"{"name":"send","kind":"send","schema_version":1,"sequence":1,"extra":true}"#;
        assert!(serde_json::from_str::<TraceRecord>(unknown).is_err());
    }
}
