use serde::{Deserialize, Serialize};

use super::ColorRole;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum FontWeight {
    Normal,
    Medium,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum FontFamily {
    Display,
    Sans,
    Mono,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FontSkin {
    pub size: f32,
    pub weight: FontWeight,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TextRoleSkin {
    pub font: FontFamily,
    pub weight: FontWeight,
    pub size: f32,
    pub spacing: f32,
    pub color: ColorRole,
}
