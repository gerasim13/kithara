<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-app-document

Workspace crate (`publish = false`) that owns how the application configuration
document is layered and baked. `kithara-app` runs the same code twice: its build
script bakes `app.yaml` into the binary, and its startup lays an overlay on the
baked document.

## Usage

```rust
use std::collections::HashMap;

let env = HashMap::from([("KITHARA_KEY".to_owned(), "secret".to_owned())]);
let baked = kithara_app_document::bake("aarch64", "key: $KITHARA_KEY\n", "key: null\n", &env)?;
assert_eq!(baked.resolved, [("KITHARA_KEY".to_owned(), "secret".to_owned())]);
# Ok::<(), serde_yaml_ng::Error>(())
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>merge</code></td><td>Lays one document on another: mappings merge key by key, every other value replaces what it covers</td></tr>

<tr><td><code>bake</code> / <code>Bake</code></td><td>The document a target embeds, the references it names and the values the build found for them</td></tr>

</table>

## Integration

`kithara-app` depends on this crate twice: as a build dependency for `bake`,
and as a normal dependency for `merge` when an overlay is loaded at startup.
