<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-app-library

Workspace crate (`publish = false`) that owns the contract between the
application's library shell and the sources it mounts. A source implements
`LibrarySource`; the shell in `kithara-app` draws its branch, rows and page,
and routes its page's reads and writes to it.

## Usage

A source crate exports a `Factory`. The application lists the factories its
build mounts and hands each one a `Context`: the shared HTTP client, the
runtime, a cancellation of its own and the source's entry of the document's
`sources` map. The factory decodes that entry with `Context::section` and
returns a `Registration`, which the shell builds once the package's text
catalog is known.

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>LibrarySource</code></td><td>A source's branch, rows, status and the reads and writes its page declares</td></tr>

<tr><td><code>SourcePage</code> / <code>Endpoint</code></td><td>The page module, documents and captions a source brings, and its endpoints, registered as <code>source.&lt;name&gt;</code> scoped by <code>source</code></td></tr>

<tr><td><code>Registration</code></td><td>A source's page and how to build the source from the text catalog</td></tr>

<tr><td><code>Factory</code> / <code>Context</code></td><td>How the application builds a source from its configuration entry</td></tr>

<tr><td><code>Playable</code></td><td>The track a row hands a deck it is dropped on, carried as the row's drag record</td></tr>

</table>

See [library sources](https://github.com/zvuk/kithara/wiki/kithara-app#library-sources)
for the contract between the shell and its sources.
