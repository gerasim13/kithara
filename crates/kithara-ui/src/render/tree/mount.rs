use iced::alignment::Horizontal;

use super::{
    atom::{
        cell, checkbox, chip, crossfader, fader, glyph, knob, meter, nav_item, readout, segmented,
        select, status_dot, swatch, tab_large, toggle, vu_stereo, vu_vertical,
    },
    geometry::Rendered,
    panel::{context_bar, deck_summary, time, track_list, tree, vis},
    read_flag, wave_zoom,
    window::{titlebar, window_controls},
};
use crate::{
    compile::CompiledUi,
    module::TextAlign,
    mount,
    render::{
        InputOwner, ReadValue, Reads, Skin,
        controls::{ButtonView, button},
        icons::document_icon,
    },
    widgets::{
        Widget,
        deck::Bpm,
        global_bar::{Brand, Divider, PresetSelector, SettingsButton, Spacer},
        telemetry::Telemetry,
        text::Text,
        wave::mini::MiniWave,
        window::WindowSurface,
    },
};

/// How one built-in control becomes an element of the immediate-mode tree.
///
/// Every control answers for itself, so the host walks the document and hands
/// each one the same surroundings instead of keeping a table of what each
/// control is made of.
pub(super) trait ViewControl {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a>;
}

/// What a control is handed when it mounts: the document it was read from, the
/// value behind it, and who owns the pointer over it.
pub(super) struct Cx<'a, 'reads, 'value> {
    pub(super) owner: InputOwner,
    pub(super) path: &'a str,
    pub(super) reads: &'reads dyn Reads,
    pub(super) scope: &'a str,
    pub(super) skin: &'a Skin,
    pub(super) ui: &'a CompiledUi,
    pub(super) value: Option<&'value ReadValue<'reads>>,
}

impl ViewControl for mount::Summary {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(deck_summary(
            self.style, cx.value, cx.scope, cx.reads, cx.skin,
        ))
    }
}

impl ViewControl for mount::Brand {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(Brand::builder().skin(cx.skin).build().view())
    }
}

impl ViewControl for mount::Spacer {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(Spacer::builder().skin(cx.skin).build().view())
    }
}

impl ViewControl for mount::Divider {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(Divider::builder().skin(cx.skin).build().view())
    }
}

impl ViewControl for mount::Preset {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(
            PresetSelector::builder()
                .reads(cx.reads)
                .skin(cx.skin)
                .build()
                .view(),
        )
    }
}

impl ViewControl for mount::Settings {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(SettingsButton::builder().skin(cx.skin).build().view())
    }
}

impl ViewControl for mount::Drag {
    fn view<'a>(&self, _cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(WindowSurface::drag().view())
    }
}

impl ViewControl for mount::TitleBar {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(titlebar(self.label, cx.ui, cx.skin))
    }
}

impl ViewControl for mount::Controls {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(window_controls(self.style, cx.skin))
    }
}

impl ViewControl for mount::Text<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::new(
            Text::builder()
                .style(self.style)
                .maybe_value(cx.value)
                .maybe_label(self.label.map(|id| cx.ui.resolve(id)))
                .maybe_color(self.color)
                .maybe_active_color(self.active_color)
                .active(read_flag(self.active, cx.reads, cx.ui))
                .skin(cx.skin)
                .build()
                .view(),
            horizontal(self.align),
        )
    }
}

impl ViewControl for mount::Glyph<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(glyph(
            self.icon,
            self.active_icon,
            self.style,
            self.color,
            self.active_color,
            read_flag(self.active, cx.reads, cx.ui),
            cx.skin,
        ))
    }
}

impl ViewControl for mount::NavItem {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(nav_item(
            cx.path,
            cx.ui.resolve(self.label),
            self.icon,
            cx.value,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Tab {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(tab_large(
            cx.path,
            cx.ui.resolve(self.label),
            cx.value,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Button {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(button(&ButtonView {
            active_label: self.active_label.map(|id| cx.ui.resolve(id)),
            frame: self.frame,
            icon: self.icon.map(document_icon),
            label: cx.ui.resolve(self.label),
            owner: cx.owner,
            path: cx.path,
            skin: cx.skin,
            style: self.style,
            value: cx.value,
        }))
    }
}

impl ViewControl for mount::Bpm {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(
            Bpm::builder()
                .maybe_placeholder(self.placeholder.map(|id| cx.ui.resolve(id)))
                .maybe_value(cx.value)
                .scope(cx.scope)
                .reads(cx.reads)
                .skin(cx.skin)
                .build()
                .view(),
        )
    }
}

impl ViewControl for mount::Time {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(time(cx.value, cx.scope, cx.reads, cx.skin))
    }
}

impl ViewControl for mount::Telemetry {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(
            Telemetry::builder()
                .format(self.format)
                .framed(self.framed)
                .maybe_value(cx.value)
                .skin(cx.skin)
                .build()
                .view(),
        )
    }
}

impl ViewControl for mount::Crossfader {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(crossfader(cx.path, self.ticks, cx.value, cx.skin, cx.owner))
    }
}

impl ViewControl for mount::Fader {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(fader(
            cx.path, self.style, self.label, cx.value, cx.ui, cx.skin, cx.owner,
        ))
    }
}

impl ViewControl for mount::Wave<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        let wave = MiniWave::builder()
            .path(cx.path)
            .style(self.style)
            .zoom(wave_zoom(self.zoom, cx.reads, cx.ui))
            .maybe_badge(self.badge.map(|id| cx.ui.resolve(id)))
            .maybe_value(cx.value)
            .scope(cx.scope)
            .reads(cx.reads)
            .skin(cx.skin)
            .build();
        Rendered::leading(match cx.owner {
            InputOwner::Leaf => wave.view(),
            InputOwner::Engine => wave.painted(),
        })
    }
}

impl ViewControl for mount::Vis {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(vis(cx.value, cx.reads))
    }
}

impl ViewControl for mount::TrackList<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(track_list(
            cx.path,
            (self.columns, self.columns_state),
            cx.value,
            cx.ui,
            cx.reads,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Tree<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(tree(
            cx.path, self.query, cx.value, cx.ui, cx.reads, cx.skin, cx.owner,
        ))
    }
}

impl ViewControl for mount::ContextBar<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(context_bar(
            cx.path,
            (self.scope_items, self.scope),
            cx.value,
            cx.ui,
            cx.reads,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Toggle {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(toggle(cx.path, cx.value, cx.skin, cx.owner))
    }
}

impl ViewControl for mount::Checkbox {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(checkbox(cx.path, cx.value, cx.skin, cx.owner))
    }
}

impl ViewControl for mount::Segmented<'_> {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(segmented(
            cx.path, self.items, cx.value, cx.ui, cx.skin, cx.owner,
        ))
    }
}

impl ViewControl for mount::Select {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(select(self.label, cx.ui, cx.skin))
    }
}

impl ViewControl for mount::StatusDot {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(status_dot(self.label, self.tone, cx.ui, cx.skin))
    }
}

impl ViewControl for mount::Swatch {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(swatch(self.role, self.label, cx.ui, cx.skin))
    }
}

impl ViewControl for mount::Cell {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(cell(self.label, self.highlighted, cx.ui, cx.skin))
    }
}

impl ViewControl for mount::Readout {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(readout(
            self.label,
            self.tone,
            self.framed,
            cx.value,
            cx.ui,
            cx.skin,
        ))
    }
}

impl ViewControl for mount::Chip {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(chip(
            cx.path,
            cx.ui.resolve(self.label),
            self.style,
            cx.value,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Knob {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(knob(
            cx.path,
            self.label.map(|id| cx.ui.resolve(id)),
            cx.value,
            cx.skin,
            cx.owner,
        ))
    }
}

impl ViewControl for mount::Meter {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(meter(cx.value, cx.skin))
    }
}

impl ViewControl for mount::VuStereo {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(vu_stereo(cx.path, cx.value, cx.skin, cx.owner))
    }
}

impl ViewControl for mount::VuVertical {
    fn view<'a>(&self, cx: &Cx<'a, '_, '_>) -> Rendered<'a> {
        Rendered::leading(vu_vertical(
            cx.path, self.ticks, cx.value, cx.skin, cx.owner,
        ))
    }
}

fn horizontal(align: TextAlign) -> Horizontal {
    match align {
        TextAlign::Start => Horizontal::Left,
        TextAlign::Center => Horizontal::Center,
        TextAlign::End => Horizontal::Right,
    }
}
