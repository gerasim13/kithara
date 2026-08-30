use std::ops::Index;

use crate::{
    draw::Rgba,
    error::UiDocError,
    ids::SourceUri,
    skin::{ColorRole, PaletteDoc, color_roles, parse_color},
};

const CHANNEL_MAX: f32 = 255.0;

macro_rules! define_render_palette {
    ($($field:ident => $role:ident),* $(,)?) => {
        /// Resolved color palette consumed by renderers.
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[non_exhaustive]
        pub struct RenderPalette {
            $(pub $field: Rgba,)*
        }

        impl RenderPalette {
            /// Reads every role the document declared into the color a
            /// renderer paints with.
            ///
            /// # Errors
            /// Returns [`UiDocError`] when a value is not a valid color.
            pub(crate) fn resolve(
                document: &PaletteDoc,
                origin: &SourceUri,
            ) -> Result<Self, UiDocError> {
                Ok(Self {
                    $($field: color(&document.$field, origin)?,)*
                })
            }
        }

        impl Index<ColorRole> for RenderPalette {
            type Output = Rgba;

            fn index(&self, role: ColorRole) -> &Rgba {
                match role {
                    $(ColorRole::$role => &self.$field,)*
                }
            }
        }
    };
}

color_roles!(define_render_palette);

pub(crate) fn color(value: &str, origin: &SourceUri) -> Result<Rgba, UiDocError> {
    let [red, green, blue, alpha] = parse_color(value, origin)?;
    Ok(Rgba {
        r: f32::from(red) / CHANNEL_MAX,
        g: f32::from(green) / CHANNEL_MAX,
        b: f32::from(blue) / CHANNEL_MAX,
        a: f32::from(alpha) / CHANNEL_MAX,
    })
}
