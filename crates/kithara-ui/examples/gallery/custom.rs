use std::{convert::Infallible, sync::LazyLock};

use kithara_ui::{
    builtin,
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform},
    render::{
        CustomSkin,
        custom::{CustomKinds, CustomWidget, Size2, SizeLimits, TextMeasurer},
    },
    source::UiConfig,
};
use num_traits::cast::AsPrimitive;

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
    const BARS: f32 = 12.0;
    /// What the tallest bar is drawn in when the skin dresses this kind in
    /// nothing. Every colour here is the widget's own last word, taken only
    /// when the skin says nothing about it.
    const BAR_HIGH: Rgba = Rgba {
        a: 1.0,
        b: 0.85,
        g: 0.55,
        r: 0.20,
    };
    const BAR_LOW: Rgba = Rgba {
        a: 1.0,
        b: 0.30,
        g: 0.55,
        r: 0.20,
    };
    const CAPTION: &'static str = "DRAWN BY THE APPLICATION";
    const GAP: f32 = 6.0;
    const GROUND: Rgba = Rgba {
        a: 1.0,
        b: 0.10,
        g: 0.08,
        r: 0.07,
    };
    const INK: Rgba = Rgba {
        a: 1.0,
        b: 0.72,
        g: 0.70,
        r: 0.68,
    };
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

    fn paint(
        &mut self,
        list: &mut DrawListBuilder,
        text: &mut TextMeasurer<'_>,
        bounds: Rect,
        skin: &CustomSkin,
    ) {
        let role = builtin::skin_doc().text.section;
        let run = text.shape(Self::CAPTION, role, Some(bounds.w));
        list.fill_rounded_rect(bounds, 4.0, skin.color("ground").unwrap_or(Self::GROUND));
        list.text(
            &run,
            Self::CAPTION,
            Transform::translate(Pt {
                x: bounds.x + Self::PAD,
                y: bounds.y + Self::PAD,
            }),
            skin.color("ink").unwrap_or(Self::INK),
        );
        let low = skin.color("bar_low").unwrap_or(Self::BAR_LOW);
        let high = skin.color("bar_high").unwrap_or(Self::BAR_HIGH);
        let top = (bounds.y + Self::PAD * 2.0 + run.height()).round();
        let floor = bounds.y + bounds.h - Self::PAD;
        let room = (floor - top).max(0.0);
        let bars = skin.number("bars").unwrap_or(Self::BARS).max(1.0);
        let span = (bounds.w - Self::PAD * 2.0 - Self::GAP * (bars - 1.0)).max(0.0) / bars;
        for index in 0..bars.whole() {
            let step = AsPrimitive::<f32>::as_(index + 1) / bars;
            let left = (bounds.x + Self::PAD + AsPrimitive::<f32>::as_(index) * (span + Self::GAP))
                .round();
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
                mix(low, high, step),
            );
        }
    }
}

/// The colour a bar of this height is drawn in: the skin names the two ends of
/// the ladder and every bar between them is read off the line joining them.
fn mix(low: Rgba, high: Rgba, step: f32) -> Rgba {
    Rgba {
        a: (high.a - low.a).mul_add(step, low.a),
        b: (high.b - low.b).mul_add(step, low.b),
        g: (high.g - low.g).mul_add(step, low.g),
        r: (high.r - low.r).mul_add(step, low.r),
    }
}

/// How many bars a count written in points asks for.
trait Whole {
    fn whole(self) -> usize;
}

impl Whole for f32 {
    fn whole(self) -> usize {
        AsPrimitive::as_(self)
    }
}
