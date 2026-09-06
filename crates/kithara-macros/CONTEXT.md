# kithara-macros - Context

## Ownership

This crate owns the shape of a configuration document. `#[derive(Patch)]` is the
only way a `<X>Patch` type enters the workspace: a hand-written patch struct or
a hand-written merge is a defect, not a style choice.

The crate owns no runtime state and depends on nothing but `syn`, `quote`, and
`proc-macro2`. The generated code names `::serde` and `::core` only, so a
consuming crate needs `serde` and nothing else.

## What The Derive Emits

From a configuration struct, taking every field not carrying `#[patch(skip)]`,
the derive emits two items in the same module:

- the patch struct, named after the configuration with a `Patch` suffix,
  carrying the configuration's own visibility, deriving `Clone`, `Debug`,
  `Default` and `Deserialize`, and attributed `#[serde(default,
  deny_unknown_fields)]` and `#[non_exhaustive]`;
- an inherent `apply` on the configuration itself, taking that patch by value
  and repeating the configuration's own generics and where-clause;
- for a configuration that declares `#[patch(fallible)]`, a third item: the
  patch error, named with a `PatchError` suffix, carrying one variant per way
  the merge can be refused.

The patch carries no generic parameters of its own. That is the whole reason the
derive exists: `struct-patch`, the crate this replaced, copies a struct's
generics onto the patch it generates, so a patch of a generic configuration
whose generic-carrying fields are skipped has a type parameter no field uses and
does not compile. Every configuration in this workspace is generic over its
pools, its stream, or its resampler backend, and none of those is a document
key.

## Field Mapping

- A field of type `T` becomes `Option<T>`; the merge writes it when the document
  named it.
- A field already of type `Option<T>` stays `Option<T>`, so a document names the
  value bare. An absent key is the only way to leave the caller's value
  standing: a patch cannot clear a field back to `None`.
- `#[patch(nested)]` makes the patch field `<T>Patch` and the merge recurse.
  Nesting is declared, not inferred, so a document's shape is readable from the
  configuration alone.
- `#[patch(wire = <type>, from = <path>)]` gives the key a type of its own. Use
  it when the field holds something a document cannot spell — a live handle, or
  an enum one of whose variants carries one. The key parses as the wire type and
  the merge writes `#from(value)`, so the choice a document may make is a type
  in its own right and the variants it may not name are refused by name rather
  than dropped in silence. Both halves are required; `wire` and `nested` are
  mutually exclusive, because a replaced type has no nested patch to recurse
  into.
- `doc` and `cfg` attributes carry over to the patch field, and a `cfg` also
  gates the merge statement, so a feature-gated field stays gated on both sides.

## Merges That Can Refuse

Most configurations accept any combination of their own field types, and their
`apply` returns nothing. A configuration whose invariants span its fields
declares that its merge can refuse, and `apply` returns `Result<(),
<X>PatchError>` instead:

- `#[patch(validate = <path>, error = <type>)]` on the struct names a
  `fn(Self) -> Result<Self, E>` — the one gate every route into the type holds,
  the fallible constructor included. The merge builds a whole candidate beside
  the caller's value, puts it through that gate, and commits only a judged one,
  so a refused document leaves the caller holding exactly what it had. The
  refusal reaches the patch error as `Invalid`.
- `#[patch(nested, fallible)]` on a field says the nested configuration judges
  itself. Its refusal reaches the parent under a variant named after the field,
  and the error `Display`s as `"<key>: <what the child said>"`, so a nested
  refusal reads as the path a document would have to fix. Every fallible child
  is judged before any key is written, so here too a refused document commits
  nothing.

The signature is the struct's declaration, never a consequence of the fields the
current features leave standing. `cfg` is resolved before a derive runs, so a
gated fallible field is invisible here; inferring fallibility from what is left
would move `apply`'s signature from build to build. A struct therefore carries
`#[patch(fallible)]` itself, and a field marked `fallible` without it is
refused. When the features gate every fallible key out, the patch error is
still emitted, uninhabited — the callers do not change shape.

## Security Contract

A generated patch is `Deserialize` and never `Serialize`. By the time a document
is typed its `$ENV` references are resolved, so the patch holds secrets in the
clear; serializing one would write them out. The derive emits no `Serialize`,
and adding one to a configuration must not add one to its patch.
