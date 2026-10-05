#[cfg(not(any(feature = "iced", feature = "masonry")))]
pub(crate) mod geometry;
#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host;

#[cfg(not(any(feature = "iced", feature = "masonry")))]
pub(crate) use geometry as config;
#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) use host as config;

use crate::{
    expand::Binding,
    ids::InternId,
    module::{IconName, TableColumn, TableFrame, TextAlign},
    skin::{ColorRole, FontFamily, FontWeight},
};

type TextPresentation<'a> = (
    Option<InternId>,
    Option<ColorRole>,
    Option<ColorRole>,
    Option<&'a Binding>,
    TextAlign,
    Option<FontFamily>,
    Option<FontWeight>,
);

type GlyphPresentation<'a> = (
    IconName,
    Option<IconName>,
    Option<ColorRole>,
    Option<ColorRole>,
    Option<&'a Binding>,
);

type TablePresentation<'a> = (
    &'a [TableColumn],
    Option<&'a Binding>,
    Option<&'a Binding>,
    TableFrame,
    Option<&'a Binding>,
);

/// Dispatches every document control to its owned mount configuration.
///
/// The variant/type mapping stays single. Constructors project the document
/// properties into either host presentation or hostless geometry records.
/// Each caller supplies its own bound: neutral sizing or the selected host.
macro_rules! controls {
    ($spec:expr, $mount:expr) => {{
        let with = $mount;
        match $spec {
            $crate::expand::ControlSpec::DeckSummary { style, .. } => {
                with.apply(&$crate::mount::config::deck::summary(*style))
            }
            $crate::expand::ControlSpec::Brand => with.apply(&$crate::mount::Brand),
            $crate::expand::ControlSpec::Spacer => with.apply(&$crate::mount::Spacer),
            $crate::expand::ControlSpec::Divider => with.apply(&$crate::mount::Divider),
            $crate::expand::ControlSpec::PresetSelector => with.apply(&$crate::mount::Preset),
            $crate::expand::ControlSpec::SettingsButton => with.apply(&$crate::mount::Settings),
            $crate::expand::ControlSpec::WindowDrag => with.apply(&$crate::mount::Drag),
            $crate::expand::ControlSpec::TitleBar { label, .. } => {
                with.apply(&$crate::mount::config::window::title_bar(*label))
            }
            $crate::expand::ControlSpec::WindowControls { style, .. } => {
                let control = $crate::mount::Controls::builder().style(*style);
                with.apply(&control.build())
            }
            $crate::expand::ControlSpec::Text {
                style,
                label,
                color,
                active_color,
                active,
                align,
                font,
                weight,
                ..
            } => with.apply(&$crate::mount::config::label::text(
                *style,
                (
                    *label,
                    *color,
                    *active_color,
                    active.as_ref(),
                    *align,
                    *font,
                    *weight,
                ),
            )),
            $crate::expand::ControlSpec::Glyph {
                icon,
                active_icon,
                style,
                color,
                active_color,
                active,
                ..
            } => with.apply(&$crate::mount::config::label::glyph(
                *style,
                (*icon, *active_icon, *color, *active_color, active.as_ref()),
            )),
            $crate::expand::ControlSpec::NavItem { label, icon, .. } => {
                with.apply(&$crate::mount::config::press::nav_item(*label, *icon))
            }
            $crate::expand::ControlSpec::TabLarge { label, .. } => {
                with.apply(&$crate::mount::config::press::tab(*label))
            }
            $crate::expand::ControlSpec::Button {
                label,
                icon,
                active_label,
                style,
                frame,
                ..
            } => with.apply(&$crate::mount::config::press::button(
                *style,
                (*label, *icon, *active_label, *frame),
            )),
            $crate::expand::ControlSpec::Bpm { placeholder, .. } => {
                with.apply(&$crate::mount::config::deck::bpm(*placeholder))
            }
            $crate::expand::ControlSpec::Time => with.apply(&$crate::mount::Time),
            $crate::expand::ControlSpec::Scalar { format, framed, .. } => {
                with.apply(&$crate::mount::config::label::telemetry(*format, *framed))
            }
            $crate::expand::ControlSpec::Crossfader { ticks, .. } => {
                with.apply(&$crate::mount::config::scalar::crossfader(*ticks))
            }
            $crate::expand::ControlSpec::Fader { style, label, .. } => {
                with.apply(&$crate::mount::config::scalar::fader(*style, *label))
            }
            $crate::expand::ControlSpec::Wave {
                style, badge, zoom, ..
            } => with.apply(&$crate::mount::config::deck::wave(
                *style,
                *badge,
                zoom.as_ref(),
            )),
            $crate::expand::ControlSpec::Vis => with.apply(&$crate::mount::Vis),
            $crate::expand::ControlSpec::Shader(spec) => {
                with.apply(&$crate::mount::Shader::new(spec))
            }
            $crate::expand::ControlSpec::Custom { kind } => {
                with.apply(&$crate::mount::Custom::new(*kind))
            }
            $crate::expand::ControlSpec::Lottie {
                artwork,
                active_artwork,
                active,
                seconds,
                ..
            } => with.apply(&$crate::mount::config::panel::lottie(
                *artwork,
                *active_artwork,
                active.as_ref(),
                *seconds,
            )),
            $crate::expand::ControlSpec::Sprite { sheet, seconds, .. } => {
                with.apply(&$crate::mount::config::panel::sprite(*sheet, *seconds))
            }
            $crate::expand::ControlSpec::PortalMap => with.apply(&$crate::mount::PortalMap),
            $crate::expand::ControlSpec::Range => with.apply(&$crate::mount::Range),
            $crate::expand::ControlSpec::Table {
                columns,
                columns_state,
                status,
                frame,
                width,
                ..
            } => with.apply(&$crate::mount::config::panel::table((
                columns,
                columns_state.as_ref(),
                status.as_ref(),
                *frame,
                width.as_ref(),
            ))),
            $crate::expand::ControlSpec::Search => with.apply(&$crate::mount::Search),
            $crate::expand::ControlSpec::Tree {
                query,
                search,
                toggle,
                ..
            } => with.apply(&$crate::mount::config::panel::tree(
                query.as_ref(),
                *search,
                *toggle,
            )),
            $crate::expand::ControlSpec::ContextBar {
                scope_items, scope, ..
            } => with.apply(&$crate::mount::config::panel::context_bar(
                scope_items,
                scope.as_ref(),
            )),
            $crate::expand::ControlSpec::Toggle => with.apply(&$crate::mount::Toggle),
            $crate::expand::ControlSpec::Checkbox => with.apply(&$crate::mount::Checkbox),
            $crate::expand::ControlSpec::Segmented { items, .. } => {
                with.apply(&$crate::mount::config::press::segmented(items))
            }
            $crate::expand::ControlSpec::Select { label, .. } => {
                with.apply(&$crate::mount::config::label::select(*label))
            }
            $crate::expand::ControlSpec::StatusDot {
                label,
                dot_size,
                tone,
                active_tone,
                active,
                ..
            } => with.apply(&$crate::mount::config::badge::status_dot(
                *label,
                (*dot_size, *tone, *active_tone, active.as_ref()),
            )),
            $crate::expand::ControlSpec::Swatch { role, label, .. } => {
                with.apply(&$crate::mount::config::badge::swatch(*role, *label))
            }
            $crate::expand::ControlSpec::Cell {
                label, highlighted, ..
            } => with.apply(&$crate::mount::config::badge::cell(*label, *highlighted)),
            $crate::expand::ControlSpec::Readout {
                label,
                tone,
                framed,
                ..
            } => with.apply(&$crate::mount::config::label::readout(
                *label, *tone, *framed,
            )),
            $crate::expand::ControlSpec::Chip { label, style, .. } => {
                with.apply(&$crate::mount::config::press::chip(*label, *style))
            }
            $crate::expand::ControlSpec::Knob { label, .. } => {
                with.apply(&$crate::mount::config::scalar::knob(*label))
            }
            $crate::expand::ControlSpec::Meter => with.apply(&$crate::mount::Meter),
            $crate::expand::ControlSpec::VuStereo => with.apply(&$crate::mount::VuStereo),
            $crate::expand::ControlSpec::VuVertical { ticks, .. } => {
                with.apply(&$crate::mount::config::scalar::vu_vertical(*ticks))
            }
        }
    }};
}

pub(crate) use controls;
