# Rust Shape

Use this for Rust idiom, naming, visibility, file shape, and error
quality. `AGENTS.md` keeps only the rules that stop a design before code.

## Standard Traits First

Before adding a named helper, check whether the shape belongs to a standard trait:

- `From`, `TryFrom`, `FromStr`
- `Display`, `Default`
- `Iterator`, `IntoIterator`, `FromIterator`
- `Read`, `Write`

Avoid inherent `to_string`, ad-hoc `from_*`, manual parser APIs, and custom
collection builders when the trait expresses the contract.

## Reuse Before Building

Prefer an established, well-maintained library over a custom implementation, and
prefer an existing workspace type over a new one. Justify a new dependency in the
task, plan, or PR description.

## Loop Shape

- Use iterator adapters for filtering, mapping, searching, and folding.
- Use `for` when the body owns side effects, early exit, ordered protocol steps,
  or stateful coordination.
- Avoid accumulator loops, `for` plus `if` plus `push`, flag loops, parallel
  collection passes, and manual `match` forwarding instead of `?`.

```rust
let active: Vec<_> = items
    .iter()
    .filter(|item| item.is_active())
    .map(Item::id)
    .collect();
```

## Types And Ownership

- Carry domain meaning with newtypes, enums, config, or builders instead of bool
  flags, primitive spreads, or loose strings.
- Choose the API ownership boundary deliberately. Do not clone only to satisfy the
  compiler.
- Prefer typed errors and `?`; manual `match` is for domain work, not forwarding.

```rust
let media = parse_media(input)?;
let stream = open_stream(media)?;
```

```rust
let session = Session::new(SessionConfig {
    timeout,
    cancel,
    pools,
});
```

## Imports And Qualified Paths

- Keep `use` imports at the top of the file. Do not place `use` inside
  functions, methods, or blocks.
- Prefer short readable names in the body over repeated deep qualified paths.
- Full paths are acceptable only when they clearly improve readability, such as
  resolving a name conflict.

## Naming

- Choose the simplest and shortest names that still describe the real meaning.
- Prefer standard, obvious words such as `open`, `new`, `get`, `put`, `read`,
  `write`, `seek`, `stream`, `send`, and `recv` when they fit.
- Avoid clever or overly long names that encode implementation history instead
  of meaning.

## Comments And Documentation

- A comment lives as a doc comment on the item it documents, or it does not
  live. Inline `//` is reserved for machine markup: `SAFETY:` on an `unsafe`
  block, and the `ast-grep-ignore:` / `xtask-lint-ignore:` directives. There is
  no prose marker, because a prose marker only labels the comment it should have
  removed. `comment_hygiene` enforces this.
- So an explanation has three honest destinations and no fourth: the item's
  `///`, the shape of the code, or a test. Prose does not fix unclear code - if
  a comment is needed to follow the logic, rename, split, or retype until it is
  not, and a comment inside a function body almost always means the body wanted
  a named function.
- Documentation earns its place by being dense. A doc block runs to a dozen
  lines at most; past that it is a document, and a document belongs in the
  owning crate `README.md`. No banner or separator comments, and no comment
  block at the top of a file.
- An invariant is pinned by a test, not by a paragraph: a test fails when the
  invariant breaks, and a paragraph silently starts lying. Only what neither the
  code shape nor a test can carry belongs in the owning crate `CONTEXT.md`, and
  `README.md` stays an overview that points to it.

## File Size And Decomposition

- Do not let a single `.rs` file grow into a dump of abstractions.
- Extract large types, big `impl` blocks, or distinct subsystems into their own
  files or modules.
- Prefer `mod.rs` plus focused sibling files over one oversized source file.
- `lib.rs` and `mod.rs` contain only module declarations and re-exports.

## Visibility And API Surface

- New items are `pub(crate)` unless they are part of a documented public
  contract. Do not use bare `pub` for internal helpers.
- Public enums and public named-field structs exposed across crate boundaries
  are `#[non_exhaustive]`. Small, obviously stable exceptions are allowed when
  extension is unlikely and direct construction is part of the contract.
- When promoting an item to `pub`, verify that it is intentional, documented,
  and covered by tests.
- Before introducing a new shared type, search the workspace and reuse an
  existing canonical type when possible.

## Generic Programming

- Prefer standard and `tokio` abstractions when they fit instead of inventing
  near-identical custom traits.
- Extend behavior through type parameters, traits, and composition instead of
  copy-paste specialization. Do not create near-duplicate HLS and file types
  when the difference can be expressed generically.
- Avoid large "god traits". Prefer several smaller traits with clearer
  ownership.

## Errors And Diagnostics

- Errors are typed and carry context about what failed and on which resource.
- Logs are useful and never leak secrets.
- Use `tracing` fields for context such as `asset_id`, `url`, `resource`,
  `variant`, `segment_idx`, `bytes`, `attempt`, or `timeout_ms`.
- Do not leave temporary prints in production code.
