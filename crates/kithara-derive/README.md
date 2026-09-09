<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-derive.svg)](https://crates.io/crates/kithara-derive)
[![docs.rs](https://docs.rs/kithara-derive/badge.svg)](https://docs.rs/kithara-derive)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-derive

Proc-macro crate for Kithara's production code. It provides `#[derive(Patch)]`:
from one configuration struct it generates `<Struct>Patch`, the shape a
configuration document may say about it, and the `apply` that merges one onto
the other. A crate keeps a single configuration struct; the patch beside it is
generated, never written.

## Usage

```rust
use kithara_derive::Patch;

#[derive(Patch)]
pub struct HlsConfig<S> {
    /// The caller hands this over; a document cannot name it.
    #[patch(skip)]
    pub store: AssetStore<S>,
    /// Max segments to download per step.
    pub download_batch_size: usize,
    /// Max bytes the downloader may run ahead of the reader.
    pub look_ahead_bytes: Option<u64>,
}

let patch: HlsConfigPatch = serde_yaml_ng::from_str("download_batch_size: 5\n")?;
config.apply(patch);
```

### Bounded Scalars

`Ranged` accepts one attribute on a concrete numeric tuple newtype:
`#[ranged(min = <literal>, max = <literal>, default = <literal>, clamp)]`.
The bounds are required; `default` and `clamp` are optional. Bounds use float
literals for `f32`/`f64` and integer literals for integer fields, optionally
negated. Generics, repeated keys, and bounds outside their declared order are
refused.

```rust
use kithara_derive::Ranged;

#[derive(Clone, Copy, Debug, PartialEq, Ranged)]
#[ranged(min = -24.0, max = 6.0, default = 0.0, clamp)]
struct Gain(f32);

assert_eq!(Gain::from(f32::NAN), Gain::DEFAULT);
assert_eq!(Gain::checked(7.0), None);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ranged)]
#[ranged(min = 0, max = 100, default = 100)]
struct Share(u8);

assert_eq!(Share::checked(101), None);
assert_eq!(u8::from(Share::default()), 100);
```

`checked` and `Deserialize` always refuse invalid values. Only `clamp` adds
`From<primitive>`; it requires `default`, which receives a floating-point NaN.
Without `default`, neither `DEFAULT` nor `Default` exists. The inverse `From`
always unwraps the value. Declaring crates must depend on `serde`; `Serialize`
is never generated.

## Key Types

Derive macros:

- `#[derive(Patch)]` — generates `<Struct>Patch` and `<Struct>::apply`
- `#[derive(Ranged)]` — generates bounded scalar construction and deserialization

Field attributes:

- `#[patch(skip)]` — the field is not a document key. Naming it is refused by
  name rather than dropped silently.
- `#[patch(nested)]` — the field's own type has a patch; the document names it
  under a key of the same name and the merge recurses.
- `#[patch(attribute(...))]` — one attribute added to the generated patch field
  alone, for example `serde(with = "humantime_serde::option")` on a `Duration`.

## Integration

Used by every crate that owns a configuration a document may reach, and by
`kithara-app`, which deserializes the generated patches out of `app.yaml` and
applies them onto the configurations it built.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-derive) for the contract the generated code keeps.
