use std::ops::Range;

use iced::{
    Element,
    advanced::{layout::Layout, mouse},
    alignment::Horizontal,
};
use num_traits::cast::AsPrimitive;

use super::{
    atom::{
        cell, checkbox, chip, crossfader, fader, glyph, knob, meter, nav_item, readout, segmented,
        select, status_dot, swatch, tab_large, toggle, vu_stereo, vu_vertical,
    },
    geometry::Rendered,
    host::{picker_input_layout, tree_input_layout},
    panel::{context_bar, deck_summary, time, track_list, tree, vis},
    read::{read_flag, read_scope, resolve, wave_zoom},
    track_list::TrackListHost,
    window::{titlebar, window_controls},
};
use crate::{
    compile::CompiledUi,
    engine::{Descriptor, Engine, ScrollConfig, Target},
    expand::{Binding, ControlSpec},
    ids::InternId,
    interact::{CursorShape, Hover, ScrollAxis, iced as iced_interact, recognizers::WheelStep},
    module::{FaderStyle, TextAlign, WaveStyle},
    render::{
        InputOwner, ReadValue, Reads, Skin, UiEvent,
        controls::{ButtonView, button, nav_item_supports_engine_input, supports_engine_input},
        icons::document_icon,
        model::derived,
        picker_selected_index,
    },
    widgets::{
        Widget,
        deck::Bpm,
        fader::fader_input_layout,
        global_bar::{Brand, Divider, PresetSelector, SettingsButton, Spacer},
        telemetry::Telemetry,
        text::Text,
        track_list::column_layouts,
        wave::{
            mini::MiniWave,
            zoom_math::{clamp_zoom, window_bounds, zoom_for_wheel},
        },
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

pub(super) enum HostedControl {
    Activation {
        path: String,
    },
    Segmented {
        path: String,
        item_count: usize,
    },
    Picker {
        path: String,
        item_count: usize,
        item_height: f32,
        selected: Option<usize>,
    },
    Scroll {
        path: String,
        config: ScrollConfig,
    },
    TrackList(Box<TrackListHost>),
    Fader {
        path: String,
        style: FaderStyle,
        labelled: bool,
        drag_step: Option<f64>,
        wheel: Option<WheelStep>,
    },
    Crossfader {
        path: String,
    },
    Knob {
        path: String,
        current: f32,
        drag_range: f32,
        wheel_step: f32,
    },
    StereoMeter {
        path: String,
    },
    VerticalVu {
        path: String,
    },
    Wave {
        path: String,
    },
    HeroWave {
        path: String,
        scale: f32,
        progress: f32,
        visible: Range<f32>,
        wheel_positive: f32,
        wheel_non_positive: f32,
    },
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
        match (spec, value) {
            (ControlSpec::Button { style, icon, .. }, _)
                if supports_engine_input(*style, icon.map(document_icon)) =>
            {
                Some(Self::Activation {
                    path: path.to_owned(),
                })
            }
            (ControlSpec::NavItem { icon, .. }, Some(ReadValue::Bool(_)))
                if nav_item_supports_engine_input(document_icon(*icon)) =>
            {
                Some(Self::Activation {
                    path: path.to_owned(),
                })
            }
            (
                ControlSpec::TabLarge { .. }
                | ControlSpec::Toggle
                | ControlSpec::Checkbox
                | ControlSpec::Chip { .. },
                Some(ReadValue::Bool(_)),
            ) => Some(Self::Activation {
                path: path.to_owned(),
            }),
            (ControlSpec::Segmented { items }, Some(ReadValue::Scalar(_))) if !items.is_empty() => {
                Some(Self::Segmented {
                    path: path.to_owned(),
                    item_count: items.len(),
                })
            }
            (ControlSpec::ContextBar { scope_items, scope }, Some(ReadValue::Text(_)))
                if !scope_items.is_empty() =>
            {
                let scope_value = scope
                    .as_ref()
                    .and_then(|binding| resolve(reads, binding, ui));
                let selected = picker_selected_index(scope_value.as_ref(), scope_items.len());
                Some(Self::Picker {
                    path: path.to_owned(),
                    item_count: scope_items.len(),
                    item_height: skin.tree.scope_item_height,
                    selected,
                })
            }
            (ControlSpec::Tree { .. }, Some(ReadValue::Tree(rows))) => Some(Self::Scroll {
                path: path.to_owned(),
                config: ScrollConfig::items(
                    ScrollAxis::Vertical,
                    AsPrimitive::<f32>::as_(rows.len()) * skin.tree.row_height,
                    rows.len(),
                    skin.tree.row_height,
                    skin.tree.row_height,
                    skin.tree.scrollbar_margin + skin.tree.scrollbar_width,
                ),
            }),
            (
                ControlSpec::TrackList {
                    columns,
                    columns_state,
                },
                Some(ReadValue::TrackList(rows)),
            ) => {
                let state = columns_state
                    .as_ref()
                    .map(|binding| (ui.resolve(binding.id), read_scope(Some(binding), ui)));
                let columns = column_layouts(columns, reads, state, skin);
                Some(Self::TrackList(Box::new(TrackListHost::new(
                    path,
                    columns,
                    rows.len(),
                    skin,
                ))))
            }
            (ControlSpec::Fader { style, label }, Some(ReadValue::Scalar(value))) => {
                let (drag_step, wheel) = match style {
                    FaderStyle::Default => (Some(skin.fader.step), None),
                    FaderStyle::Volume => (
                        None,
                        Some(WheelStep {
                            value: value.clamp(0.0, 1.0).as_(),
                            step: skin.fader.step.as_(),
                        }),
                    ),
                };
                Some(Self::Fader {
                    path: path.to_owned(),
                    style: *style,
                    labelled: label.is_some(),
                    drag_step,
                    wheel,
                })
            }
            (ControlSpec::Crossfader { .. }, Some(ReadValue::Scalar(_))) => {
                Some(Self::Crossfader {
                    path: path.to_owned(),
                })
            }
            (ControlSpec::Knob { .. }, Some(ReadValue::Scalar(value))) => Some(Self::Knob {
                path: path.to_owned(),
                current: value.clamp(0.0, 1.0).as_(),
                drag_range: skin.knob.drag_range,
                wheel_step: skin.knob.wheel_step,
            }),
            (ControlSpec::VuStereo, Some(ReadValue::Stereo(_))) => Some(Self::StereoMeter {
                path: path.to_owned(),
            }),
            (ControlSpec::VuVertical { .. }, Some(ReadValue::Stereo(_))) => {
                Some(Self::VerticalVu {
                    path: path.to_owned(),
                })
            }
            (ControlSpec::Wave { style, zoom, .. }, Some(ReadValue::Waveform(waveform)))
                if !waveform.buckets.is_empty() =>
            {
                if *style != WaveStyle::Hero {
                    return Some(Self::Wave {
                        path: path.to_owned(),
                    });
                }
                let progress = match reads.get(&derived("deck.playback.position_normalized", scope))
                {
                    Some(ReadValue::Scalar(value)) => value.as_(),
                    _ => 0.0,
                };
                let scale = clamp_zoom(wave_zoom(zoom.as_ref(), reads, ui));
                Some(Self::HeroWave {
                    path: path.to_owned(),
                    scale,
                    progress,
                    visible: window_bounds(progress, scale),
                    wheel_positive: zoom_for_wheel(scale, 1.0),
                    wheel_non_positive: zoom_for_wheel(scale, 0.0),
                })
            }
            _ => None,
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::Activation { path }
            | Self::Segmented { path, .. }
            | Self::Picker { path, .. }
            | Self::Scroll { path, .. }
            | Self::Fader { path, .. }
            | Self::Crossfader { path }
            | Self::Knob { path, .. }
            | Self::StereoMeter { path }
            | Self::VerticalVu { path }
            | Self::Wave { path }
            | Self::HeroWave { path, .. } => path,
            Self::TrackList(track_list) => track_list.path(),
        }
    }

    fn input_layout<'a>(&self, layout: Layout<'a>) -> Option<Layout<'a>> {
        match self {
            Self::Fader {
                style, labelled, ..
            } => fader_input_layout(layout, *style, *labelled),
            Self::Picker { .. } => picker_input_layout(layout),
            Self::Scroll { .. } => tree_input_layout(layout),
            _ => Some(layout),
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
    let Some(layout) = control.input_layout(layout) else {
        return;
    };
    if let HostedControl::TrackList(track_list) = control {
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
    match control {
        HostedControl::Activation { path } => {
            descriptors.push(Descriptor::activation(path.clone()));
        }
        HostedControl::Segmented { path, item_count } => {
            descriptors.push(Descriptor::segmented(path.clone(), *item_count));
        }
        HostedControl::Picker {
            path,
            item_count,
            selected,
            ..
        } => {
            descriptors.push(Descriptor::picker(path.clone(), *item_count, *selected));
        }
        HostedControl::Scroll { path, config } => {
            descriptors.push(Descriptor::scroll(path.clone(), *config));
        }
        HostedControl::TrackList(track_list) => track_list.append_descriptors(descriptors),
        HostedControl::Fader {
            path,
            style,
            drag_step,
            wheel,
            ..
        } => descriptors.push(Descriptor::fader(
            path.clone(),
            Hover::new(match style {
                FaderStyle::Default => CursorShape::Grab,
                FaderStyle::Volume => CursorShape::ResizeH,
            }),
            *drag_step,
            *wheel,
        )),
        HostedControl::Crossfader { path } => {
            descriptors.push(Descriptor::crossfader(path.clone()));
        }
        HostedControl::Knob {
            path,
            current,
            drag_range,
            wheel_step,
        } => descriptors.push(Descriptor::knob(
            path.clone(),
            *current,
            *drag_range,
            *wheel_step,
        )),
        HostedControl::StereoMeter { path } => {
            descriptors.push(Descriptor::stereo_meter(path.clone()));
        }
        HostedControl::VerticalVu { path } => {
            descriptors.push(Descriptor::vertical_vu(path.clone()));
        }
        HostedControl::Wave { path } => descriptors.push(Descriptor::wave(path.clone())),
        HostedControl::HeroWave {
            path,
            scale,
            progress,
            visible,
            wheel_positive,
            wheel_non_positive,
        } => descriptors.push(Descriptor::hero_wave(
            path.clone(),
            *scale,
            *progress,
            visible.clone(),
            *wheel_positive,
            *wheel_non_positive,
        )),
    }
}
