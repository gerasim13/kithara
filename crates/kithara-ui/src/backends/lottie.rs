//! The neutral Lottie emitter against the renderer it was written from.
//!
//! The pairing rule an artwork's draws follow lives in a private structure
//! inside velato, so no public call can be asked whether this reimplementation
//! of it agrees. Pixels can: the same artwork at the same frame is painted
//! twice into the same rasteriser, once by velato itself and once by the
//! neutral list, and the two pictures are compared.

use kithara_test_utils::kithara;
use velato::{Composition, Renderer as LottieRenderer};
use vello::{Scene, kurbo::Affine, peniko::color::palette};

use super::{VelloBackend, conformance::rasterise_at};
use crate::{
    draw::{DrawListBuilder, replay},
    lottie::emit::emit,
};

/// One artwork with a merged pair of contours under one fill, a stroked
/// open contour, a ramp, a turned layer and a layer opacity — one of each
/// thing the pairing rule and the alpha fold decide.
const PROBE: &str = include_str!("../../assets/lottie/probe.json");

fn artwork() -> Composition {
    Composition::from_slice(PROBE.as_bytes())
        .unwrap_or_else(|error| panic!("the probe artwork must read: {error}"))
}

/// Both pictures are taken on the artwork's own canvas, so nothing is
/// scaled on the way to a pixel.
fn painted(scene: &Scene) -> Vec<u8> {
    rasterise_at(scene, (200, 200), palette::css::BLACK)
        .unwrap_or_else(|error| panic!("vello must rasterise: {error}"))
}

/// What velato paints, which is the answer this emitter is measured against.
fn oracle(frame: f64) -> Vec<u8> {
    let mut scene = Scene::new();
    LottieRenderer::new().append(&artwork(), frame, Affine::IDENTITY, 1.0, &mut scene);
    painted(&scene)
}

/// What the neutral list paints, through the backend the application uses.
fn seam(frame: f64) -> Vec<u8> {
    let mut list = DrawListBuilder::default();
    emit(&artwork(), frame, &mut list)
        .unwrap_or_else(|error| panic!("the probe artwork must draw: {error}"));
    let mut scene = Scene::new();
    replay(&list.finish(), &mut VelloBackend::new(&mut scene));
    painted(&scene)
}

/// How many pixels the two pictures disagree about, at all.
///
/// No tolerance: the two paths reach the same rasteriser through different
/// arithmetic, `f64` through velato and `f32` through this list, and still
/// land on the same bytes. A shade of slack here would let a wrong pairing
/// rule hide behind antialiasing.
fn apart(frame: f64) -> usize {
    let (oracle, seam) = (oracle(frame), seam(frame));

    assert_eq!(oracle.len(), seam.len(), "the two pictures are one size");
    oracle
        .chunks(4)
        .zip(seam.chunks(4))
        .filter(|(left, right)| left != right)
        .count()
}

#[kithara::test]
fn the_neutral_list_paints_what_velato_paints() {
    assert_eq!(apart(0.0), 0);
}

/// Halfway through, where a turned layer's own matrix is what the two
/// have to agree about rather than the shapes alone.
#[kithara::test]
fn the_two_agree_partway_through_the_artwork() {
    assert_eq!(apart(30.0), 0);
}
