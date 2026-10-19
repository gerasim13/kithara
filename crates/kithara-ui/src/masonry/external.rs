use masonry::vello::{
    Renderer,
    peniko::{ImageAlphaType, ImageData, ImageFormat},
    wgpu,
    wgpu::{Origin3d, TextureAspect, TextureDimension, TextureFormat, TextureUsages},
};

use crate::draw::{Image, ImageId};

/// One externally rendered picture and the stable Vello token its producer binds.
///
/// It holds no CPU pixels. A custom widget hands it out from
/// [`CustomWidget::external_image`]; each frame that samples it lists it once in
/// [`Frame::external_images`]. Before rendering that scene the host binds
/// [`Self::image_data`] on the renderer's device and keeps the texture alive while a
/// submitted scene uses it. Vello aborts on a listed token nothing binds, so a host
/// whose producer is not ready binds an `Rgba8Unorm` placeholder of the declared
/// extent, as the window and capture hosts do. A new extent needs a new declaration.
///
/// [`CustomWidget::external_image`]: crate::render::custom::CustomWidget::external_image
/// [`Frame::external_images`]: crate::app::Frame::external_images
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

/// Cleared textures the window and capture hosts bind for device pictures.
///
/// Neither host has a producer to ask for a picture's texture, and Vello
/// aborts on a declared token nothing binds. Each picture a frame declares is
/// bound to a cleared texture of its extent instead, so it draws nothing, as
/// these hosts drew a device picture before a widget could declare one. A
/// token the frame no longer declares is released.
#[derive(Default)]
pub(crate) struct Placeholders {
    bound: Vec<ExternalImageDeclaration>,
}

impl Placeholders {
    /// Binds every picture of `declared` that is not bound yet and releases
    /// the ones it no longer lists. Call it before rendering that frame.
    pub(crate) fn bind(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut Renderer,
        declared: &[ExternalImageDeclaration],
    ) {
        for stale in self.bound.extract_if(.., |bound| !declared.contains(bound)) {
            drop(renderer.override_image(stale.image_data(), None));
        }
        for declaration in declared {
            if self.bound.contains(declaration) {
                continue;
            }
            tracing::warn!(
                image = ?declaration.image(),
                "this host has no producer for a device picture; it draws a cleared placeholder"
            );
            let data = declaration.image_data();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("kithara_ui.external.placeholder"),
                size: wgpu::Extent3d {
                    width: data.width,
                    height: data.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            drop(renderer.override_image(
                data,
                Some(wgpu::TexelCopyTextureInfoBase {
                    texture,
                    mip_level: 0,
                    origin: Origin3d::ZERO,
                    aspect: TextureAspect::All,
                }),
            ));
            self.bound.push(declaration.clone());
        }
    }
}
