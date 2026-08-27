/// Declares one skin section together with the patch that overlays it.
///
/// A skin document names every field of every section it declares, which is
/// what keeps a typo an error rather than a silent default. A patch is the
/// same section with every field optional, so a second skin can restate only
/// what it changes and inherit the rest.
macro_rules! skin_section {
    (
        $(#[$meta:meta])*
        pub struct $name:ident => $patch:ident {
            $($(#[$field_meta:meta])* pub $field:ident: $type:ty,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        #[non_exhaustive]
        pub struct $name {
            $($(#[$field_meta])* pub $field: $type,)*
        }

        #[doc = concat!("What a skin may restate of [`", stringify!($name), "`].")]
        #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
        #[serde(default, deny_unknown_fields)]
        #[non_exhaustive]
        pub struct $patch {
            $(pub $field: Option<$type>,)*
        }

        impl $name {
            /// Takes every field the patch restates, keeping the rest.
            pub(crate) fn patch(&mut self, patch: $patch) {
                $(if let Some(value) = patch.$field {
                    self.$field = value;
                })*
            }
        }
    };
}

pub(crate) use skin_section;
