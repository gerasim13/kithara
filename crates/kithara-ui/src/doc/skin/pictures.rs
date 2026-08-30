use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    error::UiDocError,
    ids::SourceUri,
    source::{base_dir, join_rel},
};

/// One picture a skin carries: where the file is, and the grid it is cut on.
///
/// The grid is named rather than guessed because a sheet and a single picture
/// are the same file to everything below: one frame is a grid of one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SheetDoc {
    pub columns: u32,
    pub rows: u32,
    /// Where the picture is, relative to the skin that names it.
    pub source: String,
}

/// Every picture a skin carries, by the name a document asks for it by.
///
/// A document names a picture and never carries one, so this is where the
/// names it may use come from: change the skin and the same document draws
/// another set of pictures, which is the whole point of a skin carrying them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(transparent)]
pub struct PictureDoc {
    pub sheets: BTreeMap<String, SheetDoc>,
}

/// What a skin restates of another skin's pictures.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(transparent)]
pub struct PicturePatch {
    pub sheets: BTreeMap<String, SheetDoc>,
}

/// Rewrites every picture path to stand on its own, against the document that
/// named it.
///
/// A skin names its pictures the way it names the skin it is written over -
/// beside itself. Once one skin is written over another the two came from two
/// places, so a path is resolved while it is still next to the document that
/// spelled it, and what is kept is a path from the root that any later reader
/// can follow.
fn rebase(sheets: &mut BTreeMap<String, SheetDoc>, origin: &SourceUri) -> Result<(), UiDocError> {
    let dir = base_dir(Some(origin));
    for sheet in sheets.values_mut() {
        if sheet.source.starts_with('/') {
            return Err(UiDocError::RootEscape {
                origin: origin.clone(),
                rel: sheet.source.clone(),
            });
        }
        sheet.source = join_rel(dir, &sheet.source).ok_or_else(|| UiDocError::RootEscape {
            origin: origin.clone(),
            rel: sheet.source.clone(),
        })?;
    }
    Ok(())
}

impl PictureDoc {
    /// Resolves every picture path against the document that named it.
    ///
    /// # Errors
    /// Returns [`UiDocError::RootEscape`] when a path leaves the source root.
    pub(crate) fn rebase(&mut self, origin: &SourceUri) -> Result<(), UiDocError> {
        rebase(&mut self.sheets, origin)
    }

    /// Takes every picture the patch names, keeping the rest.
    ///
    /// A name the patch restates is the picture that name now means, which is
    /// how one skin re-draws another's spinner without listing the pictures it
    /// leaves alone.
    pub(crate) fn patch(&mut self, patch: PicturePatch) {
        self.sheets.extend(patch.sheets);
    }
}

impl PicturePatch {
    /// Resolves every picture path against the skin that restated it.
    ///
    /// # Errors
    /// Returns [`UiDocError::RootEscape`] when a path leaves the source root.
    pub(crate) fn rebase(&mut self, origin: &SourceUri) -> Result<(), UiDocError> {
        rebase(&mut self.sheets, origin)
    }
}
