use crate::{
    atoms::{
        bar::{brand::Brand, context::Context, fill::Fill, preset::Preset, settings::Settings},
        button::Button,
        chip::Chip,
        chrome::{chevron::ChromeChevron, label::ChromeLabel},
        deck::{clock::Clock, summary::Summary, tempo::Tempo},
        design::{
            cell::Cell, crossfader::Crossfader, fader::Fader, meter::Meter, segmented::Segmented,
            select::Select, status_dot::StatusDot, swatch::Swatch,
        },
        icon::glyph::Glyph,
        knob::Knob,
        label::Telemetry,
        nav_item::NavItem,
        painter::ControlPainter,
        picture::{lottie::Lottie, sprite::Sprite},
        pivot::{map::PortalMap, range::Range},
        readout::Readout,
        tab::TabLarge,
        toggle::Binary,
        vu::{StereoMeter, VerticalVu},
        wave::face::Wave,
    },
    hosts::solve::{Length, Size, length},
    shaping::TextContext,
};

pub(crate) trait PainterLength: ControlPainter {
    /// The box it asks for when the skin, rather than the row it sits in,
    /// settles an axis.
    ///
    /// A share of the row is the one length a document cannot state — `Dim` has
    /// no portion, and the portions in this repository all come from the skin —
    /// so it is said once here rather than once per host.
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
}

impl PainterLength for Brand {}

impl PainterLength for Context {}

impl PainterLength for Fill {}

impl PainterLength for Settings {}

impl PainterLength for Chip {}

impl PainterLength for ChromeChevron {}

impl PainterLength for ChromeLabel {
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        Size::new(Length::Shrink, Length::Fill)
    }
}

impl PainterLength for Clock {}

impl PainterLength for Tempo {}

impl PainterLength for Cell {}

impl PainterLength for Crossfader {}

impl PainterLength for Meter {}

impl PainterLength for Segmented {}

impl PainterLength for Select {}

impl PainterLength for Swatch {}

impl PainterLength for Glyph {}

impl PainterLength for Knob {}

impl PainterLength for NavItem {}

impl PainterLength for TabLarge {
    /// A tab is as wide as its own word: a strip of tabs is a row of headings,
    /// not a set of equal columns, so a tab that filled its share would move
    /// its neighbours whenever a word changed.
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        Self::declared_length(self.height())
    }
}

impl PainterLength for Fader {}

impl PainterLength for Button {
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        self.declared()
    }
}

impl PainterLength for StatusDot {}

impl PainterLength for Preset {
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        self.declared()
    }
}

impl PainterLength for Wave {}

impl PainterLength for Summary {
    /// A deck's headline takes the box its skin names: it is as wide as the
    /// words it holds, not a share of the row it stands in.
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        let size = self.metrics.summary_size;
        Size::new(length(size.w), length(size.h))
    }
}

impl PainterLength for Telemetry {
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        self.declared()
    }
}

impl PainterLength for Lottie {}

impl PainterLength for Sprite {}

impl PainterLength for PortalMap {}

impl PainterLength for Range {}

impl PainterLength for Readout {}

impl PainterLength for Binary {}

impl PainterLength for StereoMeter {}

impl PainterLength for VerticalVu {}

impl Preset {
    pub(crate) fn declared(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(self.metrics.selector_width),
            Length::Fixed(self.metrics.height),
        )
    }
}

impl Button {
    /// The box it asks for. Only the width is its own: every button fills the
    /// height of the row it sits in.
    pub(crate) fn declared(&self) -> Size<Length> {
        Size::new(self.width.length(), Length::Fill)
    }
}

impl Telemetry {
    /// A framed reading fills its row; a bare one is as wide as its digits.
    pub(crate) const fn declared(&self) -> Size<Length> {
        if self.framed {
            Size::new(Length::Fill, Length::Fill)
        } else {
            Size::new(Length::Shrink, Length::Fill)
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Length, Preset, Size, Telemetry};
    use crate::{builtin, module::ScalarFormat};

    /// A bare reading is as wide as its digits, so it must ask to shrink; a
    /// framed one owns its row.
    #[kithara::test]
    fn only_a_bare_reading_asks_to_shrink() {
        let skin = builtin::skin();

        assert_eq!(
            Telemetry::new(ScalarFormat::Default, false, skin)
                .declared()
                .width,
            Length::Shrink
        );
        assert_eq!(
            Telemetry::new(ScalarFormat::Default, true, skin)
                .declared()
                .width,
            Length::Fill
        );
    }

    #[kithara::test]
    fn preset_declares_its_skin_size() {
        let skin = builtin::skin();
        let painter = Preset::new(skin);

        assert_eq!(
            painter.declared(),
            Size::new(
                Length::Fixed(skin.global_bar.selector_width),
                Length::Fixed(skin.global_bar.height),
            )
        );
    }
}
