<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-ui-gallery

Workspace crate (`publish = false`) that holds every page the UI toolkit's
documents draw, the demo host that answers what those pages read, and the
harnesses that photograph the pages and compare two hosts. The `gallery` binary
opens the pages in a window; the suites beside it check the same modules.

## Usage

```bash
cargo run -p kithara-ui-gallery --bin gallery
cargo run -p kithara-ui-gallery --features masonry --bin gallery -- --host retained
cargo run -p kithara-ui-gallery --bin gallery -- --help
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>Gallery</code></td><td>The window state: which page is open and what the demo host holds</td></tr>

<tr><td><code>Args</code></td><td>What the binary is asked to do: open a host, photograph a set, or compare sets</td></tr>

<tr><td><code>Capture</code> / <code>Shot</code></td><td>One photographed page and the set a run writes</td></tr>

<tr><td><code>DemoReads</code> / <code>DemoRegistry</code></td><td>The demo model the pages read and the endpoints they write to</td></tr>

</table>

## Features

| Feature | Enables |
| --- | --- |
| `masonry` | The retained host beside the immediate one; comparing two hosts needs both |
| `perf` | Frame and page timings through hotpath, and the `page_perf` suite |
| `mock` | The demo-model setters only the suites use |

## Integration

`just test ui` runs the `gallery` and `ui_memory` suites, and `just test parity`
drives the `gallery` binary to photograph both hosts and compare them against
`parity-budget.txt`. `kithara-ui` reads the page documents under `assets/` for
its own retained-host tests.
