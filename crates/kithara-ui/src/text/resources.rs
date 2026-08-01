use std::fmt;

use kithara_platform::sync::Arc;
use parley::fontique::{Blob, Collection, CollectionOptions};
#[cfg(feature = "render")]
use skrifa::{FontRef, outline::OutlineGlyphCollection, raw::ReadError};
use thiserror::Error;

use super::FontId;

/// Failure to construct the embedded text resources.
#[derive(Clone, Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum TextError {
    #[cfg(feature = "render")]
    #[error("embedded font face {font:?} is invalid: {source}")]
    InvalidFont {
        font: FontId,
        #[source]
        source: ReadError,
    },
    #[error("embedded font face {font:?} could not be registered")]
    Registration { font: FontId },
}

#[derive(Clone)]
pub(crate) struct TextResources {
    collection: Collection,
    fonts: [FontId; 10],
    #[cfg(feature = "render")]
    outlines: Vec<OutlineGlyphCollection<'static>>,
}

impl TextResources {
    pub(crate) fn new() -> Result<Self, TextError> {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        for font in FontId::ALL {
            let data = Blob::new(Arc::new(font.bytes()));
            if collection.register_fonts(data, None).is_empty() {
                return Err(TextError::Registration { font });
            }
        }
        Ok(Self {
            collection,
            fonts: FontId::ALL,
            #[cfg(feature = "render")]
            outlines: FontId::ALL
                .into_iter()
                .map(outline_collection)
                .collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn collection(&self) -> Collection {
        self.collection.clone()
    }

    #[cfg(feature = "render")]
    pub(crate) fn outlines(&self, font: FontId) -> &OutlineGlyphCollection<'static> {
        &self.outlines[font.index()]
    }
}

impl fmt::Debug for TextResources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextResources")
            .field("fonts", &self.fonts)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TextResources {
    fn eq(&self, other: &Self) -> bool {
        self.fonts == other.fonts
    }
}

#[cfg(feature = "render")]
fn outline_collection(font: FontId) -> Result<OutlineGlyphCollection<'static>, TextError> {
    let font_ref =
        FontRef::new(font.bytes()).map_err(|source| TextError::InvalidFont { font, source })?;
    Ok(OutlineGlyphCollection::new(&font_ref))
}
