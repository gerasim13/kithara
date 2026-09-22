<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-config.svg)](https://crates.io/crates/kithara-config)
[![docs.rs](https://docs.rs/kithara-config/badge.svg)](https://docs.rs/kithara-config)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-config

`Config::values` returns an owned snapshot of a retained configuration. It does
not mutate an owner, apply prepared state, or promise realtime safety.

`#[config]` composes bon and fieldwork. Fields explicitly select `value`, `nested`,
or `skip = "reason"`. `value(Type, expression)` projects a borrowed or internal
field into an owned public value. `#[config(default)]` derives a default through
the builder. Existing `#[builder]`, `#[fieldwork]`, and `#[field]` options remain
available. On a function or impl, `#[config]` wraps the corresponding bon builder.

The attribute emits `<Name>Values` with public snapshot fields, preserving field
documentation and configuration gates. Resource generics stay on the original
owner; snapshot types must not depend on them. Domain constructors and methods
remain responsible for validation and effects.

See the [workspace architecture](https://github.com/zvuk/kithara/wiki/kithara)
for domain ownership boundaries.
