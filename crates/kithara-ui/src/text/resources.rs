use std::fmt;

use kithara_platform::sync::Arc;
use parley::fontique::{Blob, Collection, CollectionOptions};
#[cfg(feature = "render")]
use skrifa::{FontRef, raw::ReadError};
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
    fonts: [FontId; 9],
    #[cfg(feature = "render")]
    outline_fonts: [FontRef<'static>; 9],
}

impl TextResources {
    pub(super) fn new() -> Result<Self, TextError> {
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
            outline_fonts: [
                outline_font(FontId::InterRegular)?,
                outline_font(FontId::InterSemibold)?,
                outline_font(FontId::JetBrainsMonoRegular)?,
                outline_font(FontId::JetBrainsMonoMedium)?,
                outline_font(FontId::JetBrainsMonoSemibold)?,
                outline_font(FontId::SpaceGroteskRegular)?,
                outline_font(FontId::SpaceGroteskMedium)?,
                outline_font(FontId::SpaceGroteskSemibold)?,
                outline_font(FontId::SpaceGroteskBold)?,
            ],
        })
    }

    pub(super) fn collection(&self) -> Collection {
        self.collection.clone()
    }

    #[cfg(feature = "render")]
    pub(super) fn outline_font(&self, font: FontId) -> FontRef<'static> {
        self.outline_fonts[font.index()].clone()
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
fn outline_font(font: FontId) -> Result<FontRef<'static>, TextError> {
    FontRef::new(font.bytes()).map_err(|source| TextError::InvalidFont { font, source })
}
