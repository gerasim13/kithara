use std::sync::OnceLock;

#[cfg(feature = "iced")]
use iced::{
    Color, Element, Length,
    widget::{
        svg::{self, Handle as SvgHandle, Svg},
        text,
    },
};

use crate::{draw::Outline, module::IconName};

enum IconSource {
    Lucide(lucide_icons::Icon),
    Svg(&'static Art),
}

/// One icon's authored art, read into an outline the first time it is asked
/// for and kept, because a control asks for it once a frame.
struct Art {
    document: &'static str,
    outline: OnceLock<Option<Outline>>,
}

impl Art {
    const fn new(document: &'static str) -> Self {
        Self {
            document,
            outline: OnceLock::new(),
        }
    }

    fn outline(&'static self) -> Option<&'static Outline> {
        self.outline
            .get_or_init(|| match crate::draw::outline(self.document) {
                Ok(outline) => Some(outline),
                Err(error) => {
                    tracing::error!(%error, "an icon's art is not an outline this can draw");
                    None
                }
            })
            .as_ref()
    }
}

mod art {
    use super::Art;

    pub(super) static PLAY_REVERSE: Art =
        Art::new(include_str!("../../assets/icons/play-reverse.svg"));
    pub(super) static KITHARA: Art = Art::new(include_str!("../../assets/icons/kithara.svg"));
    pub(super) static ZVUK: Art = Art::new(include_str!("../../assets/icons/zvuk.svg"));
}

/// What an icon is made of, once its source has been resolved.
///
/// Both halves reach the draw list: a glyph as shaped text, an outline as a
/// filled path. Neither needs a toolkit of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mark {
    Glyph(char),
    Outline(&'static Outline),
}

impl IconName {
    pub(crate) fn lucide_glyph(self) -> Option<char> {
        match source(self) {
            IconSource::Lucide(icon) => Some(char::from(icon)),
            IconSource::Svg(_) => None,
        }
    }

    /// What this icon draws, or nothing when its art could not be read.
    pub(crate) fn mark(self) -> Option<Mark> {
        match source(self) {
            IconSource::Lucide(icon) => Some(Mark::Glyph(char::from(icon))),
            IconSource::Svg(art) => art.outline().map(Mark::Outline),
        }
    }

    /// Renders this icon with the given size and color.
    #[must_use]
    #[cfg(feature = "iced")]
    pub fn view<'a, M: 'a>(self, size: f32, color: Color) -> Element<'a, M> {
        match source(self) {
            IconSource::Lucide(icon) => text(char::from(icon).to_string())
                .font(crate::render::fonts::LUCIDE)
                .size(size)
                .color(color)
                .into(),
            IconSource::Svg(art) => Svg::new(SvgHandle::from_memory(art.document.as_bytes()))
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .style(move |_theme, _status| svg::Style { color: Some(color) })
                .into(),
        }
    }
}

fn source(icon: IconName) -> IconSource {
    match icon {
        IconName::Activity => IconSource::Lucide(lucide_icons::Icon::Activity),
        IconName::Bell => IconSource::Lucide(lucide_icons::Icon::Bell),
        IconName::Charts => IconSource::Lucide(lucide_icons::Icon::TrendingUp),
        IconName::ChevronDown => IconSource::Lucide(lucide_icons::Icon::ChevronDown),
        IconName::ChevronRight => IconSource::Lucide(lucide_icons::Icon::ChevronRight),
        IconName::ChevronUp => IconSource::Lucide(lucide_icons::Icon::ChevronUp),
        IconName::ChevronsLeft => IconSource::Lucide(lucide_icons::Icon::ChevronsLeft),
        IconName::ChevronsRight => IconSource::Lucide(lucide_icons::Icon::ChevronsRight),
        IconName::Circle => IconSource::Lucide(lucide_icons::Icon::Circle),
        IconName::Crown => IconSource::Lucide(lucide_icons::Icon::Crown),
        IconName::Clock => IconSource::Lucide(lucide_icons::Icon::Clock),
        IconName::Collection => IconSource::Lucide(lucide_icons::Icon::CircleDot),
        IconName::Disc => IconSource::Lucide(lucide_icons::Icon::Disc),
        IconName::Faders => IconSource::Lucide(lucide_icons::Icon::Sliders),
        IconName::FastForward => IconSource::Lucide(lucide_icons::Icon::FastForward),
        IconName::Folder => IconSource::Lucide(lucide_icons::Icon::Folder),
        IconName::FolderPlus => IconSource::Lucide(lucide_icons::Icon::FolderPlus),
        IconName::Gear => IconSource::Lucide(lucide_icons::Icon::Settings),
        IconName::Headphones => IconSource::Lucide(lucide_icons::Icon::Headphones),
        IconName::Home => IconSource::Lucide(lucide_icons::Icon::Home),
        IconName::Instrument => IconSource::Lucide(lucide_icons::Icon::KeyboardMusic),
        IconName::Lock => IconSource::Lucide(lucide_icons::Icon::Lock),
        IconName::LockOpen => IconSource::Lucide(lucide_icons::Icon::LockOpen),
        IconName::Maximize => IconSource::Lucide(lucide_icons::Icon::Maximize),
        IconName::Menu => IconSource::Lucide(lucide_icons::Icon::Menu),
        IconName::Monitor => IconSource::Lucide(lucide_icons::Icon::Monitor),
        IconName::MusicNote => IconSource::Lucide(lucide_icons::Icon::Music),
        IconName::Orbit => IconSource::Lucide(lucide_icons::Icon::Orbit),
        IconName::Pause => IconSource::Lucide(lucide_icons::Icon::Pause),
        IconName::Play => IconSource::Lucide(lucide_icons::Icon::Play),
        IconName::Playlist => IconSource::Lucide(lucide_icons::Icon::ListMusic),
        IconName::PlaylistAdd => IconSource::Lucide(lucide_icons::Icon::ListPlus),
        IconName::Plus => IconSource::Lucide(lucide_icons::Icon::Plus),
        IconName::Radio => IconSource::Lucide(lucide_icons::Icon::Radio),
        IconName::RefreshCw => IconSource::Lucide(lucide_icons::Icon::RefreshCw),
        IconName::Repeat => IconSource::Lucide(lucide_icons::Icon::Repeat),
        IconName::RepeatOnce => IconSource::Lucide(lucide_icons::Icon::Repeat1),
        IconName::Rewind => IconSource::Lucide(lucide_icons::Icon::Rewind),
        IconName::Save => IconSource::Lucide(lucide_icons::Icon::Save),
        IconName::Search => IconSource::Lucide(lucide_icons::Icon::Search),
        IconName::Shuffle => IconSource::Lucide(lucide_icons::Icon::Shuffle),
        IconName::SkipBack => IconSource::Lucide(lucide_icons::Icon::SkipBack),
        IconName::SkipForward => IconSource::Lucide(lucide_icons::Icon::SkipForward),
        IconName::SlidersHorizontal => IconSource::Lucide(lucide_icons::Icon::SlidersHorizontal),
        IconName::SpeakerHigh => IconSource::Lucide(lucide_icons::Icon::Volume2),
        IconName::SpeakerLow => IconSource::Lucide(lucide_icons::Icon::Volume1),
        IconName::SpeakerX => IconSource::Lucide(lucide_icons::Icon::VolumeX),
        IconName::Usb => IconSource::Lucide(lucide_icons::Icon::Usb),
        IconName::Waveform => IconSource::Lucide(lucide_icons::Icon::AudioWaveform),
        IconName::X => IconSource::Lucide(lucide_icons::Icon::X),
        IconName::ZoomIn => IconSource::Lucide(lucide_icons::Icon::ZoomIn),
        IconName::ZoomOut => IconSource::Lucide(lucide_icons::Icon::ZoomOut),
        IconName::Kithara => IconSource::Svg(&art::KITHARA),
        IconName::PlayReverse => IconSource::Svg(&art::PLAY_REVERSE),
        IconName::Zvuk => IconSource::Svg(&art::ZVUK),
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use lucide_icons::Icon as Lucide;

    use super::Mark;
    use crate::{
        draw::{FillRule, Rect},
        module::IconName,
    };

    #[kithara::test]
    fn every_app_menu_glyph_resolves_to_its_lucide_namesake() {
        let table: [(IconName, Lucide); 16] = [
            (IconName::Menu, Lucide::Menu),
            (IconName::X, Lucide::X),
            (IconName::ChevronDown, Lucide::ChevronDown),
            (IconName::Maximize, Lucide::Maximize),
            (IconName::Disc, Lucide::Disc),
            (IconName::Gear, Lucide::Settings),
            (IconName::Monitor, Lucide::Monitor),
            (IconName::Plus, Lucide::Plus),
            (IconName::RefreshCw, Lucide::RefreshCw),
            (IconName::ChevronRight, Lucide::ChevronRight),
            (IconName::Activity, Lucide::Activity),
            (IconName::SlidersHorizontal, Lucide::SlidersHorizontal),
            (IconName::Circle, Lucide::Circle),
            (IconName::Radio, Lucide::Radio),
            (IconName::FolderPlus, Lucide::FolderPlus),
            (IconName::Save, Lucide::Save),
        ];

        for (icon, lucide) in table {
            assert_eq!(
                icon.lucide_glyph(),
                Some(char::from(lucide)),
                "{icon:?} must render {lucide:?}"
            );
        }
    }

    #[kithara::test]
    fn role_named_incumbents_keep_their_own_glyphs() {
        let prohibited: [(IconName, Lucide); 5] = [
            (IconName::Faders, Lucide::SlidersHorizontal),
            (IconName::Collection, Lucide::Circle),
            (IconName::ChevronsRight, Lucide::ChevronRight),
            (IconName::Waveform, Lucide::Activity),
            (IconName::Charts, Lucide::Activity),
        ];
        let canon: [(IconName, Lucide); 5] = [
            (IconName::Faders, Lucide::Sliders),
            (IconName::Collection, Lucide::CircleDot),
            (IconName::ChevronsRight, Lucide::ChevronsRight),
            (IconName::Waveform, Lucide::AudioWaveform),
            (IconName::Charts, Lucide::TrendingUp),
        ];

        for (icon, wrong) in prohibited {
            assert_ne!(
                icon.lucide_glyph(),
                Some(char::from(wrong)),
                "{icon:?} must not be substituted by {wrong:?}"
            );
        }
        for (icon, right) in canon {
            assert_eq!(
                icon.lucide_glyph(),
                Some(char::from(right)),
                "{icon:?} must render {right:?}"
            );
        }
    }

    #[kithara::test]
    fn svg_icons_do_not_cross_the_glyph_seam() {
        assert_eq!(IconName::PlayReverse.lucide_glyph(), None);
        assert_eq!(IconName::Zvuk.lucide_glyph(), None);
    }

    /// Both authored icons read as outlines, so neither has to reach a toolkit
    /// to be seen. One of them fills with the even-odd rule and would be a solid
    /// blob without it, which is why the rule is asserted rather than assumed.
    #[kithara::test]
    fn authored_icons_read_as_outlines_this_can_draw() {
        let box_of = Rect {
            h: 1.0,
            w: 1.0,
            x: 0.0,
            y: 0.0,
        };
        for icon in [IconName::PlayReverse, IconName::Zvuk] {
            let Some(Mark::Outline(outline)) = icon.mark() else {
                panic!("{icon:?} must read as an outline");
            };
            assert!(
                outline.placed(box_of).verbs().len() > 4,
                "{icon:?} must carry its whole shape"
            );
        }

        let Some(Mark::Outline(zvuk)) = IconName::Zvuk.mark() else {
            panic!("the mark must be an outline");
        };
        assert_eq!(zvuk.placed(box_of).rule(), FillRule::EvenOdd);
    }
}
