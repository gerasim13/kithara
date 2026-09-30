use serde::{Deserialize, Serialize};

use super::{binding::BindingRef, node::ControlNode};
use crate::{
    doc::ron_io,
    envelope::{self, DocKind},
    error::UiDocError,
    ids::{DocId, SourceUri},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModuleDoc {
    #[serde(default)]
    pub chrome: ChromeStyle,
    pub root: ControlNode,
    pub id: DocId,
    #[serde(default)]
    pub chip: Option<String>,
    #[serde(default)]
    pub drop: Option<ModuleDrop>,
    #[serde(default)]
    pub footer: Option<BindingRef>,
    /// Written when the module's header is pressed to fold or unfold it.
    #[serde(default)]
    pub collapse: Option<BindingRef>,
    #[serde(default)]
    pub title: Option<String>,
    pub schema: String,
    #[serde(default)]
    pub assign: Vec<String>,
    #[serde(default)]
    pub parameters: Vec<String>,
    pub version: u32,
}

/// The module takes rows dropped on it: a row carried out of a table and let
/// go over the module writes `write` with the row's drag data.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModuleDrop {
    pub write: BindingRef,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ChromeStyle {
    Full,
    #[default]
    Frame,
    Plain,
}

/// Parses a validated module document.
///
/// # Errors
/// Returns [`UiDocError`] when the envelope or module body is invalid.
pub fn parse_module(text: &str, origin: &SourceUri) -> Result<ModuleDoc, UiDocError> {
    let envelope = envelope::probe(text, origin)?;
    if envelope.kind != DocKind::Module {
        return Err(UiDocError::WrongDocKind {
            origin: origin.clone(),
            expected: DocKind::Module.name(),
            found: envelope.kind.name(),
        });
    }
    ron_io::options()
        .from_str(text)
        .map_err(|source| UiDocError::Syntax {
            origin: origin.clone(),
            source: Box::new(source),
        })
}
