use std::{convert::Infallible, sync::LazyLock};

use kithara_ui::{
    builtin,
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::custom::{CustomKinds, CustomWidget, Size2, SizeLimits, TextMeasurer},
    source::UiConfig,
};

/// The kind the gallery document names, and the name this application
/// registers its own widget under.
pub(super) const LADDER: &str = "level-ladder";

/// What the gallery compiles its documents against.
///
/// The registry declares its own names, so the set the compiler refuses
/// unknown kinds against is the set the hosts can actually mount rather than a
/// second list kept beside it.
pub(super) fn config() -> &'static UiConfig {
    static CONFIG: LazyLock<UiConfig> =
        LazyLock::new(|| UiConfig::builder().custom_kinds(kinds().names()).build());
    &CONFIG
}

/// The extensions this application offers whichever host draws its document.
pub(super) fn kinds() -> CustomKinds {
    CustomKinds::default().with(LADDER, || Ladder, |never: Infallible| match never {})
}

/// Content the toolkit does not own: an application-drawn ladder of levels.
///
/// It reads nothing and publishes nothing, which is what makes it a fair
/// comparison between the two hosts: the only thing that could make the two
/// pictures differ is the hosts themselves.
struct Ladder;

impl Ladder {
    const BARS: usize = 12;
    const CAPTION: &'static str = "DRAWN BY THE APPLICATION";
    const GAP: f32 = 6.0;
    /// The extent this asks for on an axis the document left to it.
    const INTRINSIC: Size2 = Size2::new(240.0, 96.0);
    const PAD: f32 = 12.0;
}

impl CustomWidget for Ladder {
    /// Nothing leaves this widget, and an action type no value inhabits says
    /// so where a unit type would only imply it.
    type Action = Infallible;

    fn measure(&mut self, _text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        Self::INTRINSIC
    }

    fn paint(&mut self, list: &mut DrawListBuilder, text: &mut TextMeasurer<'_>, bounds: Rect) {
        let role = builtin::skin_doc().text.section;
        let run = text.shape(Self::CAPTION, role, Some(bounds.w));
        list.fill_rounded_rect(
            bounds,
            4.0,
            Rgba {
                a: 1.0,
                b: 0.10,
                g: 0.08,
                r: 0.07,
            },
        );
        list.text(
            &run,
            Self::CAPTION,
            Transform::translate(Pt {
                x: bounds.x + Self::PAD,
                y: bounds.y + Self::PAD,
            }),
            Rgba {
                a: 1.0,
                b: 0.72,
                g: 0.70,
                r: 0.68,
            },
        );
        let top = (bounds.y + Self::PAD * 2.0 + run.height()).round();
        let floor = bounds.y + bounds.h - Self::PAD;
        let room = (floor - top).max(0.0);
        let bars = Self::BARS.as_f32();
        let span = (bounds.w - Self::PAD * 2.0 - Self::GAP * (bars - 1.0)).max(0.0) / bars;
        for index in 0..Self::BARS {
            let step = (index + 1).as_f32() / bars;
            // Both edges are put on the grid, not the width rounded: a bar that
            // starts on a half pixel covers one pixel fewer than the same width
            // starting on a whole one, and rounding the width instead is what
            // makes two rasterisers disagree about which pixel an edge is in.
            let left = (bounds.x + Self::PAD + index.as_f32() * (span + Self::GAP)).round();
            let right = (left + span).round();
            let ceiling = (floor - room * step).round();
            list.fill_rounded_rect(
                Rect {
                    h: floor - ceiling,
                    w: right - left,
                    x: left,
                    y: ceiling,
                },
                2.0,
                Rgba {
                    a: 1.0,
                    b: 0.30 + step * 0.55,
                    g: 0.55,
                    r: 0.20,
                },
            );
        }
    }
}

/// The gallery counts in whole bars and draws in points, so the two meet here
/// rather than at every use.
trait AsF32 {
    fn as_f32(self) -> f32;
}

impl AsF32 for usize {
    fn as_f32(self) -> f32 {
        num_traits::cast::AsPrimitive::as_(self)
    }
}
