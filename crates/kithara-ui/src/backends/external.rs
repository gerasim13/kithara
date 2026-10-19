use vello::peniko::{ImageAlphaType, ImageData, ImageFormat};

use crate::draw::{Image, ImageId};

/// One externally rendered picture and the stable Vello token its producer binds.
///
/// This declaration owns no CPU pixels. Before rendering a scene containing it,
/// the host must bind its [`Self::image_data`] to a ready texture on the renderer's
/// device. Keep that binding and texture alive until every submitted scene that
/// references the token has finished using it. A new extent needs a new declaration.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ExternalImageDeclaration {
    image: Image,
    data: ImageData,
}

impl ExternalImageDeclaration {
    /// Creates a device picture with an explicit alpha convention.
    ///
    /// Returns `None` for a blank identity or an empty extent. The alpha convention
    /// must match the producer texture; it is not inferred from its format.
    #[must_use]
    pub fn new(id: ImageId, width: u32, height: u32, alpha_type: ImageAlphaType) -> Option<Self> {
        if id.as_str().trim().is_empty() || width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            image: Image::external(id, width, height),
            data: ImageData {
                alpha_type,
                width,
                height,
                data: Vec::new().into(),
                format: ImageFormat::Rgba8,
            },
        })
    }

    /// The neutral picture to append through the ordinary draw-list builder.
    #[must_use]
    pub const fn image(&self) -> &Image {
        &self.image
    }

    /// The stable token to bind to the producer's ready device texture.
    #[must_use]
    pub const fn image_data(&self) -> &ImageData {
        &self.data
    }
}
