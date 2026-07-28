use serde::{Deserialize, Serialize};

use super::{super::ron_io, SkinDoc};
use crate::{
    envelope::{self, DocKind},
    error::UiDocError,
    ids::SourceUri,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PaletteDoc {
    pub bg: String,
    pub bg_deep: String,
    pub bg_inset: String,
    pub bg_panel: String,
    pub bg_footer: String,
    pub bg_panel_2: String,
    pub bg_select: String,
    pub line: String,
    pub line_dim: String,
    pub line_inner: String,
    pub line_soft: String,
    pub text: String,
    pub text_dim: String,
    pub muted: String,
    pub accent: String,
    pub accent_strong: String,
    pub accent_soft: String,
    pub danger: String,
    pub success: String,
    pub warning: String,
    pub wave_low: String,
    pub wave_mid: String,
    pub wave_high: String,
}

impl PaletteDoc {
    fn validate(&self, origin: &SourceUri) -> Result<(), UiDocError> {
        for value in [
            &self.bg,
            &self.bg_deep,
            &self.bg_inset,
            &self.bg_panel,
            &self.bg_footer,
            &self.bg_panel_2,
            &self.bg_select,
            &self.line,
            &self.line_dim,
            &self.line_inner,
            &self.line_soft,
            &self.text,
            &self.text_dim,
            &self.muted,
            &self.accent,
            &self.accent_strong,
            &self.accent_soft,
            &self.danger,
            &self.success,
            &self.warning,
            &self.wave_low,
            &self.wave_mid,
            &self.wave_high,
        ] {
            parse_color(value, origin)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ColorRole {
    Bg,
    BgDeep,
    BgInset,
    BgPanel,
    BgFooter,
    BgPanel2,
    BgSelect,
    Line,
    LineDim,
    LineInner,
    LineSoft,
    Text,
    TextDim,
    Muted,
    Accent,
    AccentStrong,
    AccentSoft,
    Danger,
    Success,
    Warning,
    WaveLow,
    WaveMid,
    WaveHigh,
}

/// Parses and validates a complete skin document.
///
/// # Errors
/// Returns [`UiDocError`] when the envelope, body, or palette is invalid.
pub fn parse_skin(text: &str, origin: &SourceUri) -> Result<SkinDoc, UiDocError> {
    let envelope = envelope::probe(text, origin)?;
    if envelope.kind != DocKind::Skin {
        return Err(UiDocError::WrongDocKind {
            origin: origin.clone(),
            expected: DocKind::Skin.name(),
            found: envelope.kind.name(),
        });
    }
    let document: SkinDoc =
        ron_io::options()
            .from_str(text)
            .map_err(|source| UiDocError::Syntax {
                origin: origin.clone(),
                source: Box::new(source),
            })?;
    document.palette.validate(origin)?;
    Ok(document)
}

pub(crate) fn parse_color(value: &str, origin: &SourceUri) -> Result<[u8; 4], UiDocError> {
    let digits = value
        .strip_prefix('#')
        .ok_or_else(|| bad_color(origin, value))?;
    if digits.len() != 6 && digits.len() != 8 {
        return Err(bad_color(origin, value));
    }
    let component = |start| {
        let pair = digits
            .get(start..start + 2)
            .ok_or_else(|| bad_color(origin, value))?;
        u8::from_str_radix(pair, 16).map_err(|_| bad_color(origin, value))
    };
    Ok([
        component(0)?,
        component(2)?,
        component(4)?,
        if digits.len() == 8 {
            component(6)?
        } else {
            255
        },
    ])
}

fn bad_color(origin: &SourceUri, value: &str) -> UiDocError {
    UiDocError::BadColor {
        origin: origin.clone(),
        value: value.to_owned(),
    }
}
