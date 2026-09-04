use std::sync::LazyLock;

use num_traits::cast::AsPrimitive;
use velato::Composition;

/// One artwork, read once.
///
/// Reading a Lottie is parsing a document, so it is done at most once per name
/// for the life of the process and every drawing borrows the result. What a
/// frame costs is the emitting, which is per frame by nature.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct Artwork {
    #[field(get(vis = "pub(crate)"))]
    composition: Composition,
}

impl Artwork {
    /// Which frame of this artwork stands `seconds` into a pass of `pass`.
    ///
    /// Wraps, so a clock that keeps running keeps playing. A pass of nothing at
    /// all holds the first frame rather than dividing by it.
    pub(crate) fn frame_at(&self, seconds: f32, pass: f32) -> f64 {
        let frames = &self.composition.frames;
        let span = frames.end - frames.start;
        if !pass.is_finite() || pass <= 0.0 || !seconds.is_finite() || span <= 0.0 {
            return frames.start;
        }
        let through = f64::from(seconds / pass).rem_euclid(1.0);

        span.mul_add(through, frames.start)
    }

    /// The box the artwork was authored in, which is what a drawing fits into
    /// its own.
    pub(crate) fn size(&self) -> (f64, f64) {
        let width: f64 = self.composition.width.max(1).as_();
        let height: f64 = self.composition.height.max(1).as_();
        (width, height)
    }
}

/// The artwork of that name, or nothing.
///
/// An artwork the toolkit does not ship draws nothing, which is what an unbound
/// control does everywhere else.
#[must_use]
pub fn builtin_artwork(name: &str) -> Option<&'static Artwork> {
    /// The artwork the toolkit ships, named the way a document names a sprite
    /// sheet.
    const PULSE: &str = "pulse";
    /// The second artwork, so a document that switches between two of them has
    /// two to switch between.
    const SPARK: &str = "spark";

    static PULSE_ARTWORK: LazyLock<Option<Artwork>> =
        LazyLock::new(|| read(include_str!("../../assets/lottie/pulse.json")));
    static SPARK_ARTWORK: LazyLock<Option<Artwork>> =
        LazyLock::new(|| read(include_str!("../../assets/lottie/spark.json")));

    match name {
        PULSE => PULSE_ARTWORK.as_ref(),
        SPARK => SPARK_ARTWORK.as_ref(),
        _ => None,
    }
}

fn read(text: &str) -> Option<Artwork> {
    Composition::from_slice(text.as_bytes())
        .inspect_err(|error| tracing::error!(%error, "the built-in artwork did not read"))
        .ok()
        .map(|composition| Artwork { composition })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::builtin_artwork;

    fn shipped() -> &'static super::Artwork {
        builtin_artwork("pulse").expect("the toolkit ships the pulse artwork")
    }

    /// A document that switches artwork on a flag needs two that read, and two
    /// that are not the same drawing.
    #[kithara::test]
    fn the_two_shipped_artworks_are_two_drawings() {
        let spark = builtin_artwork("spark").expect("the toolkit ships the spark artwork");

        assert!(!std::ptr::eq(shipped(), spark));
    }

    #[kithara::test]
    fn an_artwork_the_toolkit_does_not_ship_is_not_found() {
        assert!(builtin_artwork("nothing-of-the-sort").is_none());
    }

    /// The whole point of a pass: a clock that keeps running keeps playing,
    /// rather than stopping on the last frame it reached.
    #[kithara::test]
    fn a_reading_a_whole_pass_later_comes_back_to_the_same_frame() {
        let artwork = shipped();

        assert_eq!(artwork.frame_at(0.0, 2.0), artwork.frame_at(2.0, 2.0));
    }

    #[kithara::test]
    fn a_reading_partway_through_a_pass_stands_at_a_later_frame() {
        let artwork = shipped();

        assert!(artwork.frame_at(1.0, 2.0) > artwork.frame_at(0.0, 2.0));
    }

    /// A pass of nothing holds the artwork's own first frame rather than
    /// dividing by it.
    #[kithara::test]
    fn a_pass_of_no_time_holds_the_first_frame() {
        let artwork = shipped();

        assert_eq!(
            artwork.frame_at(1.0, 0.0),
            artwork.composition().frames.start
        );
    }
}
