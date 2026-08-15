use masonry::vello::{
    Scene,
    peniko::{ImageAlphaType, ImageData, ImageFormat},
};

use crate::{
    backends::VelloImageBackend,
    compile::CompiledUi,
    draw::{DrawListBuilder, Rect, replay},
    render::{
        Reads,
        shader::{ShaderDeclaration, ShaderFrame, ShaderFrameError, logical_extent},
    },
    shader::ShaderSpec,
};

pub(super) struct ShaderLeaf {
    error: Option<ShaderFrameError>,
    frame: Option<ShaderFrame>,
    image: Option<ImageData>,
    path: String,
    spec: ShaderSpec,
}

impl ShaderLeaf {
    pub(super) fn new(spec: ShaderSpec, path: String, reads: &dyn Reads, ui: &CompiledUi) -> Self {
        let mut this = Self {
            error: None,
            frame: None,
            image: None,
            path,
            spec,
        };
        this.update(reads, ui);
        this
    }

    pub(super) fn declaration(&self) -> Option<ShaderDeclaration> {
        Some(ShaderDeclaration::new(
            self.frame.clone()?,
            self.image.clone()?,
        ))
    }

    pub(super) fn is_continuous(&self) -> bool {
        !self.spec.uniforms().is_empty()
    }

    pub(super) fn paint(&mut self, bounds: Rect, scene: &mut Scene) {
        let Some(frame) = &self.frame else {
            return;
        };
        let Some(size) = logical_extent(bounds.w, bounds.h) else {
            return;
        };
        if self
            .image
            .as_ref()
            .is_none_or(|image| [image.width, image.height] != size)
        {
            self.image = Some(ImageData {
                data: Vec::new().into(),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: size[0],
                height: size[1],
            });
        }
        let Some(image) = &self.image else {
            return;
        };
        let mut list = DrawListBuilder::default();
        list.image(frame.image().clone(), bounds);
        replay(
            &list.finish(),
            &mut VelloImageBackend::new(scene, frame.image(), image),
        );
    }

    pub(super) fn refresh(&mut self, reads: &dyn Reads, ui: &CompiledUi) -> bool {
        self.update(reads, ui)
    }

    fn update(&mut self, reads: &dyn Reads, ui: &CompiledUi) -> bool {
        match ShaderFrame::read(&self.spec, &self.path, reads, ui) {
            Ok(frame) => {
                self.error = None;
                self.frame.as_ref() != Some(&frame) && {
                    self.frame = Some(frame);
                    true
                }
            }
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    tracing::error!(%error, "retained document shader did not produce a frame");
                }
                self.error = Some(error);
                self.frame.take().is_some()
            }
        }
    }
}
