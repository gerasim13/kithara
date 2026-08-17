//! The neutral Lottie emitter against the renderer it was written from.
//!
//! The pairing rule an artwork's draws follow lives in a private structure
//! inside velato, so no public call can be asked whether this reimplementation
//! of it agrees. Pixels can: the same artwork at the same frame is painted
//! twice into the same rasteriser, once by velato itself and once by the
//! neutral list, and the two pictures are compared.

use std::fmt::{self, Display, Formatter};

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

fn painted(scene: &Scene) -> Vec<u8> {
    let side = u32::try_from(Apart::CANVAS).unwrap_or(u32::MAX);
    rasterise_at(scene, (side, side), palette::css::BLACK)
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

/// What the two pictures disagree about.
///
/// A count on its own names nothing when it fails. The two paths reach one
/// rasteriser through different arithmetic, `f64` through velato and `f32`
/// through this list, so a wrong pairing rule and a rounded antialiased edge
/// both read as "some pixels differ"; only the size of the disagreement, and
/// where it starts, tells them apart.
struct Apart {
    pixels: usize,
    worst: u8,
    first: Option<(usize, [u8; 4], [u8; 4])>,
}

impl Display for Apart {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let Some((at, oracle, seam)) = self.first else {
            return write!(formatter, "the two pictures are the same");
        };
        write!(
            formatter,
            "{} of {} pixels apart, worst {} of 255, first at ({}, {}): velato {oracle:?} against the list {seam:?}",
            self.pixels,
            Self::CANVAS * Self::CANVAS,
            self.worst,
            at % Self::CANVAS,
            at / Self::CANVAS,
        )
    }
}

impl Apart {
    /// The artwork's own canvas, so nothing is scaled on the way to a pixel.
    const CANVAS: usize = 200;

    /// No tolerance: the two paths land on the same bytes, and a shade of
    /// slack here would let a wrong pairing rule hide behind antialiasing.
    fn at(frame: f64) -> Self {
        let (oracle, seam) = (oracle(frame), seam(frame));
        assert_eq!(oracle.len(), seam.len(), "the two pictures are one size");

        let mut apart = Self {
            pixels: 0,
            worst: 0,
            first: None,
        };
        for (at, (left, right)) in oracle.chunks(4).zip(seam.chunks(4)).enumerate() {
            if left == right {
                continue;
            }
            apart.pixels += 1;
            let worst = left
                .iter()
                .zip(right.iter())
                .map(|(one, two)| one.abs_diff(*two))
                .max()
                .unwrap_or(0);
            apart.worst = apart.worst.max(worst);
            if apart.first.is_none() {
                let pair = |bytes: &[u8]| [bytes[0], bytes[1], bytes[2], bytes[3]];
                apart.first = Some((at, pair(left), pair(right)));
            }
        }
        apart
    }
}

#[kithara::test]
fn the_neutral_list_paints_what_velato_paints() {
    let apart = Apart::at(0.0);

    assert_eq!(apart.pixels, 0, "{apart}");
}

/// Halfway through, where a turned layer's own matrix is what the two
/// have to agree about rather than the shapes alone.
#[kithara::test]
fn the_two_agree_partway_through_the_artwork() {
    let apart = Apart::at(30.0);

    assert_eq!(apart.pixels, 0, "{apart}");
}
