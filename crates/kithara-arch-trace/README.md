<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-arch-trace.svg)](https://crates.io/crates/kithara-arch-trace)
[![docs.rs](https://docs.rs/kithara-arch-trace/badge.svg)](https://docs.rs/kithara-arch-trace)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-arch-trace

Versioned architecture trace records and a JSONL writer. Runtime scenarios depend
on this format crate without building the devtools command infrastructure.

```rust
use kithara_arch_trace::{TraceRecord, TraceWriter};

let mut writer = TraceWriter::create(std::path::Path::new("trace.jsonl"))?;
let record: TraceRecord = serde_json::from_str(
    r#"{"schema_version":1,"sequence":1,"kind":"event","name":"queue.append"}"#,
)?;
writer.write(&record)?;
writer.finish()?;
# Ok::<(), anyhow::Error>(())
```

The schema version, optional record fields, and IO errors are retained from the
architecture trace format. Devtools owns importing, correlation, source matching,
and graph rendering. See the [architecture trace contract](https://github.com/zvuk/kithara/wiki/kithara-devtools).
