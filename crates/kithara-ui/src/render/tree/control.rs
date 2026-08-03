use iced::{
    Element,
    advanced::{layout::Layout, mouse},
    alignment::Horizontal,
};

use super::{
    atom::{
        cell, checkbox, chip, crossfader, fader, glyph, knob, meter, nav_item, readout, segmented,
        select, status_dot, swatch, tab_large, toggle, vu_stereo, vu_vertical,
    },
    geometry::Rendered,
    host::{picker_input_layout, tree_input_layout, tree_search_input_layout},
    panel::{context_bar, deck_summary, time, track_list, tree, vis},
    read::{read_flag, read_scope, resolve, wave_zoom},
    track_list::TrackListHost,
    window::{titlebar, window_controls},
};
use crate::{
    compile::CompiledUi,
    engine::{Descriptor, Engine, Target},
    expand::{Binding, ControlSpec},
    ids::InternId,
    interact::iced as iced_interact,
    module::TextAlign,
    render::{
        HostedControlPlan, InputOwner, ReadValue, Reads, Skin, UiEvent,
        controls::{ButtonView, button},
        icons::document_icon,
    },
    widgets::{
        Widget,
        deck::Bpm,
        fader::fader_input_layout,
        global_bar::{Brand, Divider, PresetSelector, SettingsButton, Spacer},
        telemetry::Telemetry,
        text::Text,
        wave::mini::MiniWave,
        window::WindowSurface,
    },
};

pub(super) fn render_control<'a>(
    path: InternId,
    spec: &ControlSpec,
    read: Option<&Binding>,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
    owner: InputOwner,
) -> Rendered<'a> {
    let value = read.and_then(|binding| resolve(reads, binding, ui));
    let value = value.as_ref();
    let path = ui.resolve(path);
    let scope = read_scope(read, ui);
    let mut align = Horizontal::Left;
    let element = match spec {
        ControlSpec::DeckSummary { style } => deck_summary(*style, value, scope, reads, skin),
        ControlSpec::Brand => Brand::builder().skin(skin).build().view(),
        ControlSpec::Spacer => Spacer::builder().skin(skin).build().view(),
        ControlSpec::Divider => Divider::builder().skin(skin).build().view(),
        ControlSpec::PresetSelector => preset_selector(reads, skin),
        ControlSpec::SettingsButton => SettingsButton::builder().skin(skin).build().view(),
        ControlSpec::WindowDrag => WindowSurface::drag().view(),
        ControlSpec::TitleBar { label } => titlebar(*label, ui, skin),
        ControlSpec::WindowControls { style } => window_controls(*style, skin),
        ControlSpec::Bpm { placeholder } => {
            bpm_control(*placeholder, value, scope, ui, reads, skin)
        }
        ControlSpec::Time => time(value, scope, reads, skin),
        ControlSpec::Text {
            style,
            label,
            color,
            active_color,
            active,
            align: declared,
        } => {
            align = horizontal(*declared);
            Text::builder()
                .style(*style)
                .maybe_value(value)
                .maybe_label(label.map(|id| ui.resolve(id)))
                .maybe_color(*color)
                .maybe_active_color(*active_color)
                .active(read_flag(active.as_ref(), reads, ui))
                .skin(skin)
                .build()
                .view()
        }
        ControlSpec::Glyph {
            icon,
            active_icon,
            style,
            color,
            active_color,
            active,
        } => glyph(
            *icon,
            *active_icon,
            *style,
            *color,
            *active_color,
            read_flag(active.as_ref(), reads, ui),
            skin,
        ),
        ControlSpec::NavItem { label, icon } => {
            nav_item(path, ui.resolve(*label), *icon, value, skin, owner)
        }
        ControlSpec::TabLarge { label } => tab_large(path, ui.resolve(*label), value, skin, owner),
        ControlSpec::Button {
            label,
            icon,
            active_label,
            style,
            frame,
        } => button(&ButtonView {
            active_label: active_label.map(|id| ui.resolve(id)),
            frame: *frame,
            icon: icon.map(document_icon),
            label: ui.resolve(*label),
            owner,
            path,
            skin,
            style: *style,
            value,
        }),
        ControlSpec::Scalar { format, framed } => Telemetry::builder()
            .format(*format)
            .framed(*framed)
            .maybe_value(value)
            .skin(skin)
            .build()
            .view(),
        ControlSpec::Crossfader { ticks } => crossfader(path, *ticks, value, skin, owner),
        ControlSpec::Fader { style, label } => fader(path, *style, *label, value, ui, skin, owner),
        ControlSpec::Toggle => toggle(path, value, skin, owner),
        ControlSpec::Checkbox => checkbox(path, value, skin, owner),
        ControlSpec::Segmented { items } => segmented(path, items, value, ui, skin, owner),
        ControlSpec::Select { label } => select(*label, ui, skin),
        ControlSpec::StatusDot { label, tone } => status_dot(*label, *tone, ui, skin),
        ControlSpec::Swatch { role, label } => swatch(*role, *label, ui, skin),
        ControlSpec::Cell { label, highlighted } => cell(*label, *highlighted, ui, skin),
        ControlSpec::Readout {
            label,
            tone,
            framed,
        } => readout(*label, *tone, *framed, value, ui, skin),
        ControlSpec::Chip { label, style } => {
            chip(path, ui.resolve(*label), *style, value, skin, owner)
        }
        ControlSpec::Knob { label } => {
            knob(path, label.map(|id| ui.resolve(id)), value, skin, owner)
        }
        ControlSpec::VuStereo => vu_stereo(path, value, skin, owner),
        ControlSpec::VuVertical { ticks } => vu_vertical(path, *ticks, value, skin, owner),
        ControlSpec::Vis => vis(value, reads),
        ControlSpec::Wave { style, badge, zoom } => {
            let wave = MiniWave::builder()
                .path(path)
                .style(*style)
                .zoom(wave_zoom(zoom.as_ref(), reads, ui))
                .maybe_badge(badge.map(|id| ui.resolve(id)))
                .maybe_value(value)
                .scope(scope)
                .reads(reads)
                .skin(skin)
                .build();
            match owner {
                InputOwner::Leaf => wave.view(),
                InputOwner::Engine => wave.painted(),
            }
        }
        ControlSpec::Meter => meter(value, skin),
        ControlSpec::TrackList {
            columns,
            columns_state,
        } => track_list(
            path,
            (columns, columns_state.as_ref()),
            value,
            ui,
            reads,
            skin,
            owner,
        ),
        ControlSpec::Tree { query } => tree(path, query.as_ref(), value, ui, reads, skin, owner),
        ControlSpec::ContextBar { scope_items, scope } => context_bar(
            path,
            (scope_items, scope.as_ref()),
            value,
            ui,
            reads,
            skin,
            owner,
        ),
    };
    Rendered::new(element, align)
}

fn bpm_control<'a>(
    placeholder: Option<InternId>,
    value: Option<&ReadValue<'_>>,
    scope: &str,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    Bpm::builder()
        .maybe_placeholder(placeholder.map(|id| ui.resolve(id)))
        .maybe_value(value)
        .scope(scope)
        .reads(reads)
        .skin(skin)
        .build()
        .view()
}

fn preset_selector<'a>(reads: &dyn Reads, skin: &'a Skin) -> Element<'a, UiEvent> {
    PresetSelector::builder()
        .reads(reads)
        .skin(skin)
        .build()
        .view()
}

fn horizontal(align: TextAlign) -> Horizontal {
    match align {
        TextAlign::Start => Horizontal::Left,
        TextAlign::Center => Horizontal::Center,
        TextAlign::End => Horizontal::Right,
    }
}

pub(super) struct HostedControl {
    plan: HostedControlPlan,
    track_list: Option<Box<TrackListHost>>,
}

impl HostedControl {
    pub(super) fn new(
        path: &str,
        spec: &ControlSpec,
        value: Option<ReadValue<'_>>,
        scope: &str,
        reads: &dyn Reads,
        ui: &CompiledUi,
        skin: &Skin,
    ) -> Option<Self> {
        HostedControlPlan::resolved(path, spec, value, scope, reads, ui, skin)
            .map(|plan| Self::mounted(plan, skin))
    }

    pub(super) fn mounted(plan: HostedControlPlan, skin: &Skin) -> Self {
        let track_list = match &plan {
            HostedControlPlan::TrackList(plan) => Some(Box::new(TrackListHost::new(
                &plan.path,
                plan.columns.clone(),
                plan.row_count,
                skin,
            ))),
            _ => None,
        };
        Self { plan, track_list }
    }

    delegate::delegate! {
        to self.plan {
            fn path(&self) -> &str;
        }
    }

    fn input_layout<'a>(&self, layout: Layout<'a>) -> Option<Layout<'a>> {
        match &self.plan {
            HostedControlPlan::Fader {
                style, labelled, ..
            } => fader_input_layout(layout, *style, *labelled),
            HostedControlPlan::Picker { .. } => picker_input_layout(layout),
            _ => Some(layout),
        }
    }

    pub(super) fn picker(&self) -> Option<(&str, usize, f32)> {
        match &self.plan {
            HostedControlPlan::Picker {
                path,
                item_count,
                item_height,
                ..
            } => Some((path, *item_count, *item_height)),
            _ => None,
        }
    }
}

pub(super) fn append_control_targets<'a>(
    control: &'a HostedControl,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    engine: Option<&Engine>,
    targets: &mut Vec<Target<'a>>,
) {
    if let HostedControlPlan::Tree {
        path, search_path, ..
    } = &control.plan
    {
        if let Some(layout) = tree_search_input_layout(layout) {
            targets.push(Target::new(
                search_path,
                iced_interact::hit(layout.bounds(), cursor),
            ));
        }
        if let Some(layout) = tree_input_layout(layout) {
            targets.push(Target::new(
                path,
                iced_interact::hit(layout.bounds(), cursor),
            ));
        }
        return;
    }
    let Some(layout) = control.input_layout(layout) else {
        return;
    };
    if let Some(track_list) = &control.track_list {
        track_list.append_targets(layout, cursor, engine, targets);
    } else {
        targets.push(Target::new(
            control.path(),
            iced_interact::hit(layout.bounds(), cursor),
        ));
    }
}

pub(super) fn append_control_descriptors(
    control: &HostedControl,
    descriptors: &mut Vec<Descriptor>,
) {
    descriptors.extend(control.plan.descriptors());
}
