use core::fmt::Debug;

use crate::Config;

/// A configuration that checks every field it declares a check for.
///
/// `check(error = E)` on the struct and `check = path` on its fields generate
/// it; a nested config is validated whole, so its type is checked too.
/// ```compile_fail
/// #[derive(Clone, Copy, kithara_config::Config)]
/// #[config(builder(none))]
/// struct Plain { #[config(value)] level: u8 }
/// #[derive(Clone, Copy, kithara_config::Config)]
/// #[config(builder(none), check(error = std::io::Error))]
/// struct Outer { #[config(nested)] plain: Plain }
/// ```
pub trait CheckedConfig: Config + Sized {
    /// What a field check refuses with; a nested config's error converts into it.
    /// ```compile_fail
    /// #[derive(Clone, Copy, kithara_config::Config)]
    /// #[config(builder(none), check(error = std::io::Error))]
    /// struct Inner { #[config(value, live)] level: u8 }
    /// #[derive(Clone, Copy, kithara_config::Config)]
    /// #[config(builder(none))]
    /// struct Outer { #[config(nested, live)] inner: Inner }
    /// ```
    type Error;

    /// This config if every field check accepts its value, in declaration
    /// order, nested configs included.
    ///
    /// # Errors
    ///
    /// Returns the refusal of the first field whose check refuses its value.
    fn validated(self) -> Result<Self, Self::Error>;
}

/// A configuration whose `live` fields change one at a time while it runs.
///
/// Each live field is one variant of [`LiveConfig::Change`]; a change passes
/// its field's check alone and assigns that field alone.
/// ```compile_fail
/// #[derive(Clone, kithara_config::Config)]
/// #[config(builder(none))]
/// struct Label { #[config(value, live)] text: String }
/// ```
pub trait LiveConfig: CheckedConfig + Copy {
    /// One change of one live field.
    type Change: Copy + Debug;

    /// Whether a field is `live(owner)`, executed by the owner's own method.
    /// A nested live config has none: its parent's owner executes it whole.
    /// ```compile_fail
    /// #[derive(Clone, Copy, kithara_config::Config)]
    /// #[config(builder(none))]
    /// struct Rate { #[config(value, live(owner))] hz: u32 }
    /// #[derive(Clone, Copy, kithara_config::Config)]
    /// #[config(builder(none))]
    /// struct Deck { #[config(nested, live)] rate: Rate }
    /// ```
    const OWNER_FIELDS: bool;

    /// Assigns the field `change` names; it neither checks nor allocates.
    fn apply_change(&mut self, change: Self::Change);

    /// The change if its field's check accepts the value it carries.
    ///
    /// # Errors
    ///
    /// Returns the field check's refusal.
    fn check(change: Self::Change) -> Result<Self::Change, Self::Error>;
}
