use masonry::vello::Scene;

use crate::render::vis::VisDeclaration;

/// One complete retained frame: Vello commands followed by native Vis declarations.
#[non_exhaustive]
pub struct Frame {
    scene: Scene,
    vis: Vec<VisDeclaration>,
}

impl Frame {
    pub(super) fn new(scene: Scene, vis: Vec<VisDeclaration>) -> Self {
        Self { scene, vis }
    }

    /// The Vello scene drawn before native effects.
    #[must_use]
    pub const fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Native visualiser draws for the same frame, in logical window coordinates.
    #[must_use]
    pub fn vis(&self) -> &[VisDeclaration] {
        &self.vis
    }
}

impl From<Frame> for Scene {
    fn from(frame: Frame) -> Self {
        frame.scene
    }
}
