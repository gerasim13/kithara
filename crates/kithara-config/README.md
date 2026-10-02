<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-config.svg)](https://crates.io/crates/kithara-config)
[![docs.rs](https://docs.rs/kithara-config/badge.svg)](https://docs.rs/kithara-config)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-config

`Config` is the retained settings contract. A subsystem keeps the configuration
that governs its behavior and reads settings from it; `ConfigOwner::config`
exposes that canonical object. A mutable setting is changed in the retained
configuration, not in a second owner field. `UpdatableConfig` gives typed live
changes a common API. Rejected changes leave the accepted configuration intact.
`#[derive(ConfigOwner)]` implements the owner accessor from
`#[config_owner(field)]`. For a nested field, give its type and path as
`#[config_owner(ConfigType, field.path)]`.
With `#[config(owner_access)]`, fields marked `field(get)` also produce a
`<Config>OwnerAccess` trait. Import
that trait to call the same getter on any `ConfigOwner` of that type; the method
borrows the retained configuration directly. For an exclusively owned mutable
configuration, `#[config_owner_mut]` on the owner derive adds `ConfigOwnerMut`
and its `apply_config_update` method. Shared and realtime owners keep their
domain application method so they can prepare and publish accepted changes.
Owners still decide when to prepare and publish derived state, especially across
realtime and thread boundaries. Buffers, counters, handles and observed results
remain operational state. `Config::values` returns an owned observation snapshot;
it does not become another mutable store or promise realtime safety.

`#[derive(Config)]` builds the whole configuration type from one `#[config(...)]`
attribute. Retained fields explicitly select `value`, `nested`, or `skip = "reason"`;
`construction` classifies unmarked fields as consumed inputs. For homogeneous
retained fields, `fields(value)` or `fields(nested)` sets their default role while
explicit field roles still override it.

`value(Type, expression)` projects a borrowed or internal field into an owned
public value. The derive generates a bon builder (`X::builder()`), whose
per-field options live in the field's `builder(...)` group and whose top-level
options live in the type's; `builder(skip)` or `builder(skip = value)` leaves a
field out of the builder. `field(get)` and `field(get, copy)` add accessors; on
the type they apply to every field, with explicit field declarations overriding
that default. A generic resource can use `field(get)` to stay borrowed when the
other fields use the type's `field(get, copy)`.
`#[config(validate_builder, patch(validate = Self::check, error = Error))]`
makes `build()` return `Result<Self, Error>` through that same domain check.
`builder(existing)` keeps a domain constructor's bon builder when it must
consume inputs and retain only their prepared effective value. A fallible
`#[config(default)]` checks its declared defaults and treats their rejection
as a programming error.
`builder(none)` keeps serde-owned document schemas as retained configurations
without adding an unused programmatic constructor.
Standalone builders use `bon` directly. `Config` generates the builder for
configuration structs; a separate domain constructor annotated with `#[bon]`
remains independent of that derive.
`#[config(debug)]` derives `Debug` without the fields marked `debug(skip)`. For
a projected field stored in a wrapper, `wrap(default = value, with = Wrapper::new)`
derives the builder default and setter conversion. `Patch` reads the type's and
each field's `patch(...)` group.

`#[config(update)]` opts a retained configuration into typed runtime changes;
each writable field also uses `#[config(value, update)]`. The derive emits a
concrete update enum per property and a `<Name>Update` record. Optional values
distinguish `Set`, `Clear`, and `Unchanged`; `Reset` is emitted only when the
same field declares a builder default. A configuration that declares
`patch(validate = ..., error = ...)` stages each update and commits it only
through that check, the same gate a document merge holds; any other takes the
update in place. The derive also implements `UpdatableConfig` so owners can
apply updates through one protocol. Prepared engines and delegated live owners
keep their own explicit operations.

The derive emits `<Name>Values` with public snapshot fields, preserving field
documentation. Resource generics stay on the original owner; snapshot types
must not depend on them. Domain constructors and methods remain responsible for
preparation and effects.

See the [workspace architecture](https://github.com/zvuk/kithara/wiki/kithara)
for domain ownership boundaries.
