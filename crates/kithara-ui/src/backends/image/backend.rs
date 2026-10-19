use vello::{
    Scene,
    kurbo::{Affine, Rect as KurboRect},
    peniko::{ImageBrush, ImageData},
};

use crate::{
    backends::{
        VelloBackend,
        vello::backend::{has_system_text, placed},
    },
    draw::{Backend, Caps, DrawList, Geom, Image, Paint, Pen, Rect, Rgba, Transform, replay},
    shaping::GlyphRun,
};

/// A Vello encoder borrowing the leaf's device-image lookup for one replay.
pub(crate) struct VelloImageBackend<'scene, Lookup> {
    scene: &'scene mut Scene,
    lookup: Lookup,
}

impl<'scene, Lookup> VelloImageBackend<'scene, Lookup> {
    pub(crate) const fn new(scene: &'scene mut Scene, lookup: Lookup) -> Self {
        Self { scene, lookup }
    }
}

impl<'image, Lookup> Backend for VelloImageBackend<'_, Lookup>
where
    Lookup: FnMut(&Image) -> Option<&'image ImageData>,
{
    const CAPS: Caps = Caps::EVERYTHING;

    fn clip(&mut self, region: Rect, list: &DrawList) {
        if has_system_text(list) {
            tracing::warn!(
                issue = "vello#1198",
                ?region,
                "system-backed text inside a Vello clip may paint incorrectly"
            );
        }
        self.scene.push_clip_layer(
            Affine::IDENTITY,
            &KurboRect::new(
                f64::from(region.x),
                f64::from(region.y),
                f64::from(region.x + region.w),
                f64::from(region.y + region.h),
            ),
        );
        replay(list, self);
        self.scene.pop_layer();
    }

    fn image(&mut self, image: &Image, rect: Rect, turn: f32) {
        if image.rgba().is_some() {
            VelloBackend::new(self.scene).image(image, rect, turn);
            return;
        }
        let Some(data) = (self.lookup)(image) else {
            tracing::error!(?image, "Vello image lookup does not own this image");
            return;
        };
        if image.width() == 0
            || image.height() == 0
            || [data.width, data.height] != [image.width(), image.height()]
        {
            tracing::error!(?image, "Vello device image has an inconsistent extent");
            return;
        }
        self.scene
            .draw_image(&ImageBrush::new(data.clone()), placed(image, rect, turn));
    }

    delegate::delegate! {
        to VelloBackend::new(self.scene) {
            fn fill(&mut self, geom: &Geom, paint: Paint);
            fn stroke(&mut self, geom: &Geom, color: Rgba, pen: Pen);
            fn text(&mut self, run: &GlyphRun, content: &str, transform: Transform, color: Rgba);
        }
    }
}
