use std::ops::Range;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, overlay, renderer,
        widget::{self, Operation, Tree},
    },
    window,
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use super::{
    geometry::effective_size,
    read::{read_flag, read_scope, resolve, wave_zoom},
    track_list::TrackListHost,
};
use crate::{
    compile::CompiledUi,
    engine::{Descriptor, Engine, ScrollConfig, Target},
    expand::{ControlSpec, ExpandedNode},
    interact::{
        CursorShape, Hover, Input, ScrollAxis, iced as iced_interact, recognizers::WheelStep,
    },
    module::{ChromeStyle, FaderStyle, WaveStyle},
    render::{
        ReadValue, Reads, Skin, UiEvent,
        controls::{nav_item_supports_engine_input, supports_engine_input, sync_tree_scroll},
        engine as engine_event,
        icons::document_icon,
        model::derived,
        sync_track_list_scroll, toggle_module,
    },
    size::{Hidden, is_hidden},
    widgets::{
        fader::fader_input_layout,
        track_list::column_layouts,
        wave::zoom_math::{clamp_zoom, window_bounds, zoom_for_wheel},
    },
};

#[derive(Clone, Copy)]
pub(super) struct ModuleHost<'a> {
    pub(super) instance: &'a str,
    pub(super) module: &'a str,
    pub(super) chrome: ChromeStyle,
    pub(super) collapsed: bool,
    pub(super) drop: bool,
}

pub(super) fn module_host<'a>(
    child: Element<'a, UiEvent>,
    spec: ModuleHost<'a>,
) -> Element<'a, UiEvent> {
    Element::new(Host {
        child,
        layout: HostedLayout::module(spec),
    })
}

pub(super) fn host<'a>(
    child: Element<'a, UiEvent>,
    root: &ExpandedNode,
    ui: &CompiledUi,
    reads: &dyn Reads,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    Element::new(Host {
        child,
        layout: HostedLayout::new(root, ui, reads, skin),
    })
}

struct Host<'a> {
    child: Element<'a, UiEvent>,
    layout: HostedLayout,
}

struct State {
    engine: Engine,
    last_hovered_control: Option<String>,
    last_mouse_interaction: Option<mouse::Interaction>,
}

impl State {
    fn new(layout: &HostedLayout) -> Self {
        let mut engine = Engine::default();
        engine.reconcile(layout.descriptors());
        Self {
            engine,
            last_hovered_control: None,
            last_mouse_interaction: None,
        }
    }
}

impl IcedWidget<UiEvent, Theme, Renderer> for Host<'_> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::new(&self.layout))
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.state
            .downcast_mut::<State>()
            .engine
            .reconcile(self.layout.descriptors());
        tree.diff_children(std::slice::from_ref(&self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self
            .child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let child_layout = Layout::new(&node);
        let state = tree.state.downcast_mut::<State>();
        let targets = self.layout.targets_with_engine(
            child_layout,
            mouse::Cursor::Unavailable,
            Some(&state.engine),
        );
        state
            .engine
            .reconcile(active_descriptors(&self.layout, &targets));
        for target in &targets {
            state
                .engine
                .set_scroll_viewport(target.path, target.hit.area());
        }
        sync_scrolls(
            &mut self.child,
            &mut tree.children[0],
            child_layout,
            renderer,
            &state.engine,
            &targets,
        );
        node
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let input = iced_interact::input(event);
        let item_was_pressed = state.engine.has_pressed_item();
        let (captured, scroll_captured) = if let Some(input) = input {
            let targets = self
                .layout
                .targets_with_engine(layout, cursor, Some(&state.engine));
            if let Some(emission) = state.engine.handle(input, &targets, Instant::now()) {
                let captured = emission.outcome.is_captured();
                let scroll_captured = captured
                    && matches!(input, Input::Wheel(_))
                    && state.engine.scroll_offset(&emission.path).is_some();
                let action = self.layout.header_module(&emission.path).map_or_else(
                    || engine_event(&emission.path, emission.child, emission.outcome),
                    |module| toggle_module(module, emission.outcome.map(|_| ())),
                );
                if let Some(action) = action {
                    let (message, redraw_request, _) = action.into_inner();
                    shell.request_redraw_at(redraw_request);
                    if let Some(message) = message {
                        shell.publish(message);
                    }
                }
                (captured, scroll_captured)
            } else {
                (false, false)
            }
        } else {
            (false, false)
        };
        if !captured {
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        let item_projection_changed = item_was_pressed || state.engine.has_pressed_item();
        if scroll_captured || item_projection_changed {
            let targets = self.layout.targets_with_engine(
                layout,
                mouse::Cursor::Unavailable,
                Some(&state.engine),
            );
            sync_scrolls(
                &mut self.child,
                &mut tree.children[0],
                layout,
                renderer,
                &state.engine,
                &targets,
            );
            shell.request_redraw();
        }
        if input.is_some() && (scroll_captured || state.engine.captures_pointer()) {
            shell.capture_event();
        }

        if shell.redraw_request() != window::RedrawRequest::NextFrame {
            let targets = self
                .layout
                .targets_with_engine(layout, cursor, Some(&state.engine));
            let interaction = state.engine.cursor(&targets).into();
            let hovered = hovered_control(&targets);
            if matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
                if state.last_hovered_control.as_deref() != hovered {
                    state.last_hovered_control = hovered.map(ToOwned::to_owned);
                }
                state.last_mouse_interaction = Some(interaction);
            } else if state
                .last_mouse_interaction
                .is_some_and(|last| last != interaction)
                || state.last_hovered_control.as_deref() != hovered
            {
                shell.request_redraw();
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let interaction = interaction(
            &tree.state.downcast_ref::<State>().engine,
            &self.layout,
            layout,
            cursor,
        );
        if interaction == mouse::Interaction::None {
            self.child.as_widget().mouse_interaction(
                &tree.children[0],
                layout,
                cursor,
                viewport,
                renderer,
            )
        } else {
            interaction
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, UiEvent, Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

fn sync_scrolls(
    child: &mut Element<'_, UiEvent>,
    tree: &mut Tree,
    layout: Layout<'_>,
    renderer: &Renderer,
    engine: &Engine,
    targets: &[Target<'_>],
) {
    for target in targets {
        let Some(offset) = engine.scroll_offset(target.path) else {
            continue;
        };
        let mut sync = sync_tree_scroll(target.path, offset);
        child
            .as_widget_mut()
            .operate(tree, layout, renderer, &mut sync);
        let horizontal_path = format!("{}/scroll-x", target.path);
        let horizontal = engine.scroll_offset(&horizontal_path).unwrap_or(0.0);
        let pressed = engine.pressed_item_index(target.path);
        let mut sync = sync_track_list_scroll(target.path, horizontal, pressed, offset);
        child
            .as_widget_mut()
            .operate(tree, layout, renderer, &mut sync);
    }
}

fn interaction(
    engine: &Engine,
    layout_tree: &HostedLayout,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
) -> mouse::Interaction {
    engine
        .cursor(&layout_tree.targets_with_engine(layout, cursor, Some(engine)))
        .into()
}

fn active_descriptors(layout: &HostedLayout, targets: &[Target<'_>]) -> Vec<Descriptor> {
    layout
        .descriptors()
        .into_iter()
        .filter(|descriptor| match descriptor {
            Descriptor::Scroll { path, config } if config.axis() == ScrollAxis::Horizontal => {
                targets.iter().any(|target| target.path == path)
            }
            _ => true,
        })
        .collect()
}

fn hovered_control<'a>(targets: &[Target<'a>]) -> Option<&'a str> {
    targets
        .iter()
        .rev()
        .find(|target| target.hit.over())
        .map(|target| target.path)
}

enum HostedLayout {
    Chrome {
        drop: Option<String>,
        header: Option<(String, String)>,
        collapsed: bool,
    },
    Group {
        sized: bool,
        surfaced: bool,
        framed: bool,
        children: Vec<Self>,
    },
    Slot {
        sized: bool,
        children: Vec<Self>,
    },
    Control(Option<HostedControl>),
    SelfMeasuredControl(Option<HostedControl>),
    Passive,
}

impl HostedLayout {
    fn module(spec: ModuleHost<'_>) -> Self {
        let ModuleHost {
            instance,
            module,
            chrome,
            collapsed,
            drop,
        } = spec;
        Self::Chrome {
            drop: drop.then(|| format!("{instance}/drop")),
            header: (chrome == ChromeStyle::Full)
                .then(|| (format!("{instance}/header"), module.to_owned())),
            collapsed,
        }
    }

    fn new(node: &ExpandedNode, ui: &CompiledUi, reads: &dyn Reads, skin: &Skin) -> Self {
        let hidden: Hidden<'_> = &|block| read_flag(Some(&block.hidden), reads, ui);
        match node {
            ExpandedNode::Row {
                size,
                surface,
                frame,
                children,
                ..
            }
            | ExpandedNode::Column {
                size,
                surface,
                frame,
                children,
                ..
            } => Self::Group {
                sized: size.is_some(),
                surfaced: surface.is_some(),
                framed: frame.is_some(),
                children: children
                    .iter()
                    .filter(|child| !is_hidden(*child, hidden))
                    .map(|child| Self::new(child, ui, reads, skin))
                    .collect(),
            },
            ExpandedNode::Optional { child, .. } => Self::new(child, ui, reads, skin),
            ExpandedNode::Slot { size, children, .. } => Self::Slot {
                sized: size.is_some(),
                children: children
                    .iter()
                    .filter(|child| !is_hidden(*child, hidden))
                    .map(|child| Self::new(child, ui, reads, skin))
                    .collect(),
            },
            ExpandedNode::Control {
                path, spec, read, ..
            } => {
                let control = HostedControl::new(
                    ui.resolve(*path),
                    spec,
                    read.as_ref()
                        .and_then(|binding| resolve(reads, binding, ui)),
                    read_scope(read.as_ref(), ui),
                    reads,
                    ui,
                    skin,
                );
                if effective_size(node, skin).is_none() {
                    Self::SelfMeasuredControl(control)
                } else {
                    Self::Control(control)
                }
            }
            ExpandedNode::Popover { .. } | ExpandedNode::Pressable { .. } => Self::Passive,
        }
    }

    fn descriptors(&self) -> Vec<Descriptor> {
        let mut descriptors = Vec::new();
        self.append_descriptors(&mut descriptors);
        descriptors
    }

    fn append_descriptors(&self, descriptors: &mut Vec<Descriptor>) {
        match self {
            Self::Chrome { drop, header, .. } => {
                if let Some(path) = drop {
                    descriptors.push(Descriptor::crossing(path.clone()));
                }
                if let Some((path, _)) = header {
                    descriptors.push(Descriptor::activation(path.clone()));
                }
            }
            Self::Group { children, .. } | Self::Slot { children, .. } => {
                for child in children {
                    child.append_descriptors(descriptors);
                }
            }
            Self::Control(Some(control)) | Self::SelfMeasuredControl(Some(control)) => {
                append_control_descriptors(control, descriptors);
            }
            Self::Control(None) | Self::SelfMeasuredControl(None) | Self::Passive => {}
        }
    }

    #[cfg(test)]
    fn targets<'a>(&'a self, layout: Layout<'_>, cursor: mouse::Cursor) -> Vec<Target<'a>> {
        self.targets_with_engine(layout, cursor, None)
    }

    fn targets_with_engine<'a>(
        &'a self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        engine: Option<&Engine>,
    ) -> Vec<Target<'a>> {
        let mut targets = Vec::new();
        self.append_targets(layout, cursor, engine, &mut targets);
        targets
    }

    fn append_targets<'a>(
        &'a self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        engine: Option<&Engine>,
        targets: &mut Vec<Target<'a>>,
    ) {
        match self {
            Self::Chrome {
                drop,
                header,
                collapsed,
            } => {
                let shell = if let Some(path) = drop {
                    targets.push(Target::new(
                        path,
                        iced_interact::hit(layout.bounds(), cursor),
                    ));
                    let Some(shell) = first_child(layout) else {
                        return;
                    };
                    shell
                } else {
                    layout
                };
                let Some((path, _)) = header else {
                    return;
                };
                let Some(body) = first_child(shell) else {
                    return;
                };
                let Some(content) = first_child(body) else {
                    return;
                };
                let header = if *collapsed {
                    content
                } else {
                    let Some(header) = first_child(content) else {
                        return;
                    };
                    header
                };
                targets.push(Target::new(
                    path,
                    iced_interact::hit(header.bounds(), cursor),
                ));
            }
            Self::Group {
                sized,
                surfaced,
                framed,
                children,
            } => {
                let Some(layout) = group_children(layout, *sized, *surfaced, *framed) else {
                    return;
                };
                for (child, layout) in children.iter().zip(layout.children()) {
                    child.append_targets(layout, cursor, engine, targets);
                }
            }
            Self::Slot { sized, children } => {
                let Some(layout) = slot_children(layout, *sized) else {
                    return;
                };
                for (child, layout) in children.iter().zip(layout.children()) {
                    child.append_targets(layout, cursor, engine, targets);
                }
            }
            Self::Control(Some(control)) => {
                let Some(layout) = first_child(layout) else {
                    return;
                };
                append_control_targets(control, layout, cursor, engine, targets);
            }
            Self::SelfMeasuredControl(Some(control)) => {
                append_control_targets(control, layout, cursor, engine, targets);
            }
            Self::Control(None) | Self::SelfMeasuredControl(None) | Self::Passive => {}
        }
    }

    fn header_module<'a>(&'a self, path: &str) -> Option<&'a str> {
        match self {
            Self::Chrome {
                header: Some((header, module)),
                ..
            } if header == path => Some(module),
            Self::Chrome { .. }
            | Self::Group { .. }
            | Self::Slot { .. }
            | Self::Control(_)
            | Self::SelfMeasuredControl(_)
            | Self::Passive => None,
        }
    }
}

enum HostedControl {
    Activation {
        path: String,
    },
    Segmented {
        path: String,
        item_count: usize,
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
    fn new(
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
            Self::Scroll { .. } => tree_input_layout(layout),
            _ => Some(layout),
        }
    }
}

fn append_control_targets<'a>(
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

fn append_control_descriptors(control: &HostedControl, descriptors: &mut Vec<Descriptor>) {
    match control {
        HostedControl::Activation { path } => {
            descriptors.push(Descriptor::activation(path.clone()));
        }
        HostedControl::Segmented { path, item_count } => {
            descriptors.push(Descriptor::segmented(path.clone(), *item_count));
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

fn tree_input_layout(layout: Layout<'_>) -> Option<Layout<'_>> {
    let panel = layout.children().nth(1)?;
    first_child(panel)
}

fn group_children(
    mut layout: Layout<'_>,
    sized: bool,
    surfaced: bool,
    framed: bool,
) -> Option<Layout<'_>> {
    if sized {
        layout = first_child(layout)?;
    }
    if surfaced {
        layout = first_child(layout)?;
    }
    if framed {
        layout = first_child(first_child(layout)?)?;
    }
    first_child(layout)
}

fn slot_children(mut layout: Layout<'_>, sized: bool) -> Option<Layout<'_>> {
    if sized {
        layout = first_child(layout)?;
    }
    first_child(layout)
}

fn first_child(layout: Layout<'_>) -> Option<Layout<'_>> {
    layout.children().next()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use iced::{
        Pixels, Point, Rectangle, Size,
        advanced::{
            clipboard,
            graphics::text::font_system,
            layout::{Layout, Limits},
            widget::{Tree, tree::Tag},
        },
        mouse::{self, Button, Cursor},
        widget::{Space, container, mouse_area},
    };
    use iced_renderer::fallback::Renderer as FallbackRenderer;
    use iced_tiny_skia::Renderer as TinySkiaRenderer;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        compile::{CompiledNode, compile},
        ids::EndpointId,
        registry::{EndpointCategory, EndpointDesc, EndpointRegistry, ValueKind},
        render::{
            ControlAction, DragPhase, InputOwner, StereoLevels, TrackRow, TreeIcon, TreeRow,
            WaveBucket, WaveformView,
            fonts::{FONT_BYTES, SANS},
        },
        source::{MemResolver, UiConfig},
        widgets::{DropZone, ModuleChrome, Widget, wheel::WheelSurface},
    };

    struct Fixtures {
        track_rows: [TrackRow<'static>; 9],
        tree_rows: [TreeRow<'static>; 8],
        wave_buckets: [WaveBucket; 2],
    }

    const FIXTURES: Fixtures = Fixtures {
        track_rows: [TrackRow {
            title: "Track",
            artist: Some("Artist"),
            time: Some("04:12"),
            search: None,
            deck: Some("A"),
            bpm: Some("124.0"),
            key: Some("8A"),
            energy: Some(7),
            transition: Some("blend"),
            selected: false,
        }; 9],
        tree_rows: [TreeRow {
            depth: 1,
            label: "Row",
            icon: TreeIcon::Folder,
            count: None,
            expanded: None,
            selected: false,
            muted: false,
        }; 8],
        wave_buckets: [
            WaveBucket {
                low: 0.2,
                mid: 0.4,
                high: 0.6,
            },
            WaveBucket {
                low: 0.3,
                mid: 0.5,
                high: 0.7,
            },
        ],
    };

    struct Registry {
        boolean: EndpointDesc,
        scalar: EndpointDesc,
        scoped_scalar: EndpointDesc,
        stereo: EndpointDesc,
        text: EndpointDesc,
        track_list: EndpointDesc,
        tree: EndpointDesc,
        waveform: EndpointDesc,
    }

    impl Default for Registry {
        fn default() -> Self {
            Self {
                boolean: EndpointDesc::new(ValueKind::Bool),
                scalar: EndpointDesc::new(ValueKind::Scalar),
                scoped_scalar: EndpointDesc::new(ValueKind::Scalar).with_scope("deck"),
                stereo: EndpointDesc::new(ValueKind::Stereo),
                text: EndpointDesc::new(ValueKind::Text),
                track_list: EndpointDesc::new(ValueKind::TrackList),
                tree: EndpointDesc::new(ValueKind::Tree),
                waveform: EndpointDesc::new(ValueKind::Waveform).with_scope("deck"),
            }
        }
    }

    impl EndpointRegistry for Registry {
        fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
            match (category, id.0.as_str()) {
                (EndpointCategory::Parameter, "gain")
                | (EndpointCategory::Model | EndpointCategory::Parameter, "mock.cells.segmented")
                | (EndpointCategory::Model, "mock.volume") => Some(&self.scalar),
                (EndpointCategory::Telemetry, "levels")
                | (EndpointCategory::Model, "mock.levels") => Some(&self.stereo),
                (
                    EndpointCategory::Model,
                    "mock.toggle.on" | "mock.toggle.off" | "mock.checkbox.on" | "mock.checkbox.off"
                    | "mock.button.play" | "mock.button.cue" | "mock.button.sync"
                    | "mock.chip.active" | "mock.chip.inactive",
                ) => Some(&self.boolean),
                (
                    EndpointCategory::Model,
                    "gallery.label.meters"
                    | "gallery.label.toggles"
                    | "gallery.label.chips"
                    | "gallery.label.transport"
                    | "gallery.label.regular"
                    | "gallery.label.text"
                    | "gallery.label.faders"
                    | "gallery.label.scalar"
                    | "mock.track.title"
                    | "mock.track.artist",
                ) => Some(&self.text),
                (EndpointCategory::Model, endpoint)
                    if endpoint.starts_with("gallery.tab.")
                        || endpoint.starts_with("gallery.module.") =>
                {
                    Some(&self.boolean)
                }
                (EndpointCategory::Model, "mock.wave") => Some(&self.waveform),
                (EndpointCategory::Model, "library.visible_tracks") => Some(&self.track_list),
                (EndpointCategory::Model, "library.tree") => Some(&self.tree),
                (EndpointCategory::Model, "gallery.tracklist.preset") => Some(&self.scalar),
                (EndpointCategory::Model, endpoint)
                    if endpoint.starts_with("gallery.tracklist.columns.") =>
                {
                    if endpoint.starts_with("gallery.tracklist.columns.width.") {
                        Some(&self.scalar)
                    } else {
                        Some(&self.boolean)
                    }
                }
                (EndpointCategory::Command, "mock.seek")
                | (EndpointCategory::Telemetry, "deck.playback.position_normalized") => {
                    Some(&self.scoped_scalar)
                }
                _ => None,
            }
        }
    }

    struct FixtureReads {
        gain: f64,
        progress: f64,
    }

    impl Default for FixtureReads {
        fn default() -> Self {
            Self {
                gain: 0.5,
                progress: 0.75,
            }
        }
    }

    impl Reads for FixtureReads {
        fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
            match endpoint {
                "gain" | "mock.volume" => Some(ReadValue::Scalar(self.gain)),
                "levels" | "mock.levels" => Some(ReadValue::Stereo(StereoLevels {
                    l: 0.4,
                    r: 0.6,
                    volume: 0.5,
                })),
                "mock.toggle.on" | "mock.checkbox.on" | "mock.button.play" | "mock.button.sync" => {
                    Some(ReadValue::Bool(true))
                }
                "mock.toggle.off" | "mock.checkbox.off" | "mock.button.cue" => {
                    Some(ReadValue::Bool(false))
                }
                "mock.chip.active" => Some(ReadValue::Bool(true)),
                "mock.chip.inactive" => Some(ReadValue::Bool(false)),
                "mock.cells.segmented" => Some(ReadValue::Scalar(2.0)),
                "gallery.tracklist.preset" => Some(ReadValue::Scalar(0.0)),
                "library.visible_tracks" => Some(ReadValue::TrackList(&FIXTURES.track_rows)),
                "library.tree" => Some(ReadValue::Tree(&FIXTURES.tree_rows)),
                "gallery.label.meters" => Some(ReadValue::Text("VU / STEREO / VERTICAL")),
                "gallery.label.toggles" => Some(ReadValue::Text("TOGGLES / CHECKBOXES")),
                "gallery.label.chips" => Some(ReadValue::Text("CHIP")),
                "gallery.label.transport" => Some(ReadValue::Text("TRANSPORT")),
                "gallery.label.regular" => Some(ReadValue::Text("REGULAR")),
                "gallery.label.text" => Some(ReadValue::Text("TEXT STYLES")),
                "gallery.label.faders" => Some(ReadValue::Text("HORIZONTAL FADERS")),
                "gallery.label.scalar" => Some(ReadValue::Text("SCALAR TELEMETRY")),
                "mock.track.title" => Some(ReadValue::Text("Track")),
                "mock.track.artist" => Some(ReadValue::Text("Artist")),
                endpoint if endpoint.starts_with("gallery.tab.") => {
                    Some(ReadValue::Bool(endpoint == "gallery.tab.atoms"))
                }
                endpoint if endpoint.starts_with("gallery.module.") => {
                    Some(ReadValue::Bool(endpoint == "gallery.module.deck"))
                }
                endpoint if endpoint.starts_with("gallery.tracklist.columns.width.") => None,
                endpoint if endpoint.starts_with("gallery.tracklist.columns.") => {
                    Some(ReadValue::Bool(true))
                }
                "mock.wave@deck=a" => Some(ReadValue::Waveform(WaveformView {
                    buckets: &FIXTURES.wave_buckets,
                    beats: &[],
                    downbeats: &[],
                    bpm: None,
                    r#loop: None,
                    cues: &[],
                })),
                "deck.playback.position_normalized@deck=a" => {
                    Some(ReadValue::Scalar(self.progress))
                }
                _ => None,
            }
        }
    }

    fn compiled_fixture() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "layout.klayout.ron",
            include_str!("../../../tests/fixtures/retained_host/layout.klayout.ron"),
        );
        resolver.insert(
            "mixer.kmodule.ron",
            include_str!("../../../tests/fixtures/retained_host/mixer.kmodule.ron"),
        );
        resolver.insert(
            "studio-strip.kmodule.ron",
            include_str!("../../../tests/fixtures/retained_host/studio-strip.kmodule.ron"),
        );
        compile(
            "layout.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("retained host fixture must compile: {error}"))
    }

    fn compiled_tree_surface() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "tree.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "tree-surface-host",
                root: Module(instance: "tree", source: "tree.kmodule.ron"))"#,
        );
        resolver.insert(
            "tree.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "gallery-tree-tab",
                root: Column(
                    id: "surface",
                    write: Parameter(id: "gain"),
                    children: [
                        Tree(id: "browser", read: Model(id: "library.tree")),
                    ],
                ))"#,
        );
        compile(
            "tree.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("tree surface fixture must compile: {error}"))
    }

    fn compiled_gallery_primitive(page: &str, source: &str) -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-primitive-host",
                root: Module(instance: "atoms", source: "modules/tabs/atoms.kmodule.ron"))"#,
        );
        let tab = format!(
            r#"(schema: "kithara.module", version: 1, id: "gallery-atoms-tab",
                root: Column(children: [
                    Text(id: "intro", label: "ATOMS"),
                    Include(id: "{page}", source: "../primitives/{page}.kmodule.ron"),
                ]))"#
        );
        resolver.insert("modules/tabs/atoms.kmodule.ron", &tab);
        resolver.insert(&format!("modules/primitives/{page}.kmodule.ron"), source);
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery {page} fixture must compile: {error}"))
    }

    fn compiled_gallery_meters() -> CompiledUi {
        compiled_gallery_primitive(
            "meters",
            include_str!("../../../examples/gallery/assets/modules/primitives/meters.kmodule.ron"),
        )
    }

    fn compiled_gallery_toggles() -> CompiledUi {
        compiled_gallery_primitive(
            "toggles",
            include_str!("../../../examples/gallery/assets/modules/primitives/toggles.kmodule.ron"),
        )
    }

    fn compiled_gallery_chips() -> CompiledUi {
        compiled_gallery_primitive(
            "chips",
            include_str!("../../../examples/gallery/assets/modules/primitives/chips.kmodule.ron"),
        )
    }

    fn compiled_gallery_buttons() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-buttons-host",
                root: Module(instance: "buttons", source: "buttons.kmodule.ron"))"#,
        );
        resolver.insert(
            "buttons.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/tabs/buttons.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery buttons fixture must compile: {error}"))
    }

    fn compiled_gallery_cells() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-cells-host",
                root: Module(instance: "cells", source: "cells.kmodule.ron"))"#,
        );
        resolver.insert(
            "cells.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/tabs/cells.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery cells fixture must compile: {error}"))
    }

    fn compiled_gallery_track_list() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-track-list-host",
                root: Module(instance: "tracklist", source: "tracklist.kmodule.ron"))"#,
        );
        resolver.insert(
            "tracklist.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/tabs/tracklist.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery track-list fixture must compile: {error}"))
    }

    fn compiled_gallery_faders() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-faders-host",
                root: Module(instance: "faders", source: "faders.kmodule.ron"))"#,
        );
        resolver.insert(
            "faders.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/tabs/faders.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery faders fixture must compile: {error}"))
    }

    fn compiled_gallery_tabs() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-tabs-host",
                root: Module(instance: "modules-tabs", source: "module-tabs.kmodule.ron"))"#,
        );
        resolver.insert(
            "module-tabs.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/module-tabs.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery module tabs fixture must compile: {error}"))
    }

    fn compiled_gallery_nav() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "gallery.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "gallery-nav-host",
                root: Module(instance: "gallery", source: "modules/nav.kmodule.ron"))"#,
        );
        resolver.insert(
            "modules/nav.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/nav.kmodule.ron"),
        );
        resolver.insert(
            "modules/nav/item.kmodule.ron",
            include_str!("../../../examples/gallery/assets/modules/nav/item.kmodule.ron"),
        );
        compile(
            "gallery.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("gallery nav fixture must compile: {error}"))
    }

    fn compiled_overview_row() -> CompiledUi {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "layout.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "overview-host",
                root: Module(instance: "overview", source: "studio-overview.kmodule.ron"))"#,
        );
        resolver.insert(
            "studio-overview.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "studio-overview",
                root: Row(children: [
                    Include(
                        id: "a",
                        source: "studio-overview-row.kmodule.ron",
                        with: { "deck": "a" },
                    ),
                ]))"#,
        );
        resolver.insert(
            "studio-overview-row.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "studio-overview-row",
                parameters: ["deck"],
                root: Row(gap: 0.0, size: (w: Fill, h: Fixed(40.0)), children: [
                    Text(id: "letter", label: "A"),
                    Wave(
                        id: "wave",
                        read: Model(id: "mock.wave", with: { "deck": "$deck" }),
                        write: Command(id: "mock.seek", with: { "deck": "$deck" }),
                    ),
                    Text(id: "remain", label: "00:00"),
                ]))"#,
        );
        compile(
            "layout.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("overview row fixture must compile: {error}"))
    }

    fn headless_renderer() -> Renderer {
        let mut fonts = font_system()
            .write()
            .unwrap_or_else(|error| panic!("iced font system lock must be available: {error}"));
        for bytes in FONT_BYTES {
            fonts.load_font(Cow::Borrowed(bytes));
        }
        drop(fonts);

        FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, Pixels(14.0)))
    }

    fn host_count(tree: &Tree) -> usize {
        usize::from(tree.tag == Tag::of::<State>())
            + tree.children.iter().map(host_count).sum::<usize>()
    }

    fn claimed_components(node: &ExpandedNode, components: &mut Vec<&'static str>) {
        match node {
            ExpandedNode::Row { children, .. }
            | ExpandedNode::Column { children, .. }
            | ExpandedNode::Slot { children, .. } => {
                for child in children {
                    claimed_components(child, components);
                }
            }
            ExpandedNode::Optional { child, .. } => claimed_components(child, components),
            ExpandedNode::Control { spec, .. } => match spec {
                ControlSpec::Button { style, icon, .. }
                    if supports_engine_input(*style, icon.map(document_icon)) =>
                {
                    components.push("activation");
                }
                ControlSpec::NavItem { icon, .. }
                    if nav_item_supports_engine_input(document_icon(*icon)) =>
                {
                    components.push("activation");
                }
                ControlSpec::TabLarge { .. }
                | ControlSpec::Toggle
                | ControlSpec::Checkbox
                | ControlSpec::Chip { .. } => {
                    components.push("activation");
                }
                ControlSpec::Segmented { .. } => {
                    components.push("segmented");
                }
                ControlSpec::TrackList { .. } => {
                    components.push("track-list");
                }
                ControlSpec::Fader { .. } => {
                    components.push("fader");
                }
                ControlSpec::Knob { .. } => {
                    components.push("knob");
                }
                ControlSpec::VuStereo => {
                    components.push("stereo-meter");
                }
                ControlSpec::VuVertical { .. } => {
                    components.push("vertical-vu");
                }
                ControlSpec::Crossfader { .. } => {
                    components.push("crossfader");
                }
                ControlSpec::Wave { style, .. } => {
                    components.push(if *style == WaveStyle::Hero {
                        "hero-wave"
                    } else {
                        "wave"
                    });
                }
                ControlSpec::Tree { .. } => {
                    components.push("scroll");
                }
                _ => {}
            },
            ExpandedNode::Popover { .. } | ExpandedNode::Pressable { .. } => {}
        }
    }

    fn descriptor_path(descriptor: &Descriptor) -> &str {
        match descriptor {
            Descriptor::Activation { path }
            | Descriptor::Crossing { path }
            | Descriptor::Segmented { path, .. }
            | Descriptor::Scroll { path, .. }
            | Descriptor::ColumnDivider { path, .. }
            | Descriptor::Fader { path, .. }
            | Descriptor::Crossfader { path }
            | Descriptor::Knob { path, .. }
            | Descriptor::StereoMeter { path }
            | Descriptor::VerticalVu { path }
            | Descriptor::Wave { path }
            | Descriptor::HeroWave { path, .. } => path,
            Descriptor::Item { target, .. } => target,
        }
    }

    fn chrome_child<'a>(
        content: Element<'a, UiEvent>,
        module: &'a str,
        style: ChromeStyle,
        drop: bool,
        collapsed: bool,
    ) -> Element<'a, UiEvent> {
        ModuleChrome::builder()
            .content(content)
            .module(module)
            .assign(Vec::new())
            .style(style)
            .input_owner(InputOwner::Engine)
            .maybe_drop(drop.then(|| DropZone::new(false)))
            .collapsed(collapsed)
            .skin(builtin::skin())
            .build()
            .view()
    }

    #[kithara::test]
    fn module_drop_crossing_observes_boundaries_and_forwards_to_the_child() {
        let instance = "deck-a";
        let module = "studio-deck";
        let spec = ModuleHost {
            instance,
            module,
            chrome: ChromeStyle::Plain,
            collapsed: false,
            drop: true,
        };
        let content = mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
            .on_move(|_| UiEvent::OpenSettings)
            .on_exit(UiEvent::OpenSettings)
            .into();
        let child = chrome_child(content, module, ChromeStyle::Plain, true, false);
        let mut element = module_host(child, spec);
        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 40.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::module(spec);
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            ["deck-a/drop"],
            "the whole drop zone is the outer host's only target"
        );
        assert_eq!(targets[0].hit.area(), Rectangle::with_size(viewport).into());

        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let inside = Point::new(50.0, 20.0);
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: inside }),
            Layout::new(&node),
            Cursor::Available(inside),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "deck-a/drop".to_owned(),
                    action: ControlAction::Drag(DragPhase::Over(true)),
                },
                UiEvent::OpenSettings,
            ],
            "entry publishes once and the observed move still reaches the child"
        );

        let still_inside = Point::new(60.0, 20.0);
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved {
                position: still_inside,
            }),
            Layout::new(&node),
            Cursor::Available(still_inside),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "deck-a/drop".to_owned(),
                    action: ControlAction::Drag(DragPhase::Over(true)),
                },
                UiEvent::OpenSettings,
                UiEvent::OpenSettings,
            ],
            "an inside move produces no second crossing and still reaches the child"
        );

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorLeft),
            Layout::new(&node),
            Cursor::Unavailable,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "deck-a/drop".to_owned(),
                    action: ControlAction::Drag(DragPhase::Over(true)),
                },
                UiEvent::OpenSettings,
                UiEvent::OpenSettings,
                UiEvent::Control {
                    path: "deck-a/drop".to_owned(),
                    action: ControlAction::Drag(DragPhase::Over(false)),
                },
                UiEvent::OpenSettings,
            ],
            "exit publishes once and the observed leave still reaches the child"
        );
    }

    #[kithara::test]
    fn full_module_header_activation_toggles_the_module_directly() {
        let module = "studio-deck";
        let spec = ModuleHost {
            instance: "deck-a",
            module,
            chrome: ChromeStyle::Full,
            collapsed: false,
            drop: true,
        };
        let content = Space::new().width(Length::Fill).height(Length::Fill).into();
        let child = chrome_child(content, module, ChromeStyle::Full, true, false);
        let mut element = module_host(child, spec);
        let renderer = headless_renderer();
        let viewport = Size::new(200.0, 120.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::module(spec);
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            ["deck-a/drop", "deck-a/header"]
        );
        assert!(matches!(
            descriptors.as_slice(),
            [Descriptor::Crossing { .. }, Descriptor::Activation { .. }]
        ));
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            ["deck-a/drop", "deck-a/header"]
        );
        let header = targets[1].hit.area();
        let cursor = Cursor::Available(Point::new(
            header.x + header.w / 2.0,
            header.y + header.h / 2.0,
        ));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(
            !shell.is_event_captured(),
            "the stateless activation does not retain the engine capture slot"
        );
        drop(shell);

        assert_eq!(messages, [UiEvent::ToggleModule(module.to_owned())]);
    }

    #[kithara::test]
    fn decoded_input_unanswered_by_the_engine_reaches_the_child() {
        let child = mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(UiEvent::OpenSettings)
            .into();
        let mut element = Element::new(Host {
            child,
            layout: HostedLayout::Control(None),
        });
        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 40.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);

        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            Cursor::Available(Point::new(50.0, 20.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        drop(shell);

        assert_eq!(messages, [UiEvent::OpenSettings]);
    }

    #[kithara::test]
    fn unanswered_wheel_reaches_the_still_iced_tempo_surface_once() {
        let child = WheelSurface::builder().path("deck-a/tempo").build().view();
        let mut element = Element::new(Host {
            child,
            layout: HostedLayout::Control(None),
        });
        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 40.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);

        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            }),
            Layout::new(&node),
            Cursor::Available(Point::new(50.0, 20.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(
            shell.is_event_captured(),
            "the still-iced tempo surface owns its wheel detent"
        );
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "deck-a/tempo".to_owned(),
                action: ControlAction::StepScalar(1.0),
            }],
            "the unanswered detent must reach the child exactly once"
        );
    }

    #[kithara::test]
    fn tree_boundary_passes_downward_wheel_to_the_iced_surface_but_keeps_upward_wheel() {
        let ui = compiled_tree_surface();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("tree surface fixture root must be a module");
        };
        assert_eq!(ui.resolve(*module), "gallery-tree-tab");

        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let renderer = headless_renderer();
        let viewport = Size::new(232.0, 120.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert!(matches!(
            descriptors.as_slice(),
            [Descriptor::Scroll {
                path,
                config,
            }] if path == "tree/browser"
                && *config == ScrollConfig::items(
                    ScrollAxis::Vertical,
                    192.0,
                    8,
                    24.0,
                    24.0,
                    6.0,
                )
        ));
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let [target] = targets.as_slice() else {
            panic!("the tree document must expose exactly its scroll viewport");
        };
        let area = target.hit.area();
        let cursor = Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels {
                    x: 0.0,
                    y: -1_000.0,
                },
            }),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        let bottom = tree
            .state
            .downcast_ref::<State>()
            .engine
            .scroll_offset("tree/browser")
            .unwrap_or_else(|| panic!("the retained tree must own an offset"));
        assert!(bottom > 0.0);
        assert!(
            messages.is_empty(),
            "engine-owned scrolling emits no UiEvent"
        );

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            }),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "tree/surface".to_owned(),
                action: ControlAction::StepScalar(1.0),
            }],
            "a downward wheel at the tree boundary must continue to the iced ancestor"
        );

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            }),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages.len(),
            1,
            "the movable tree must keep the upward wheel"
        );
        assert!(
            tree.state
                .downcast_ref::<State>()
                .engine
                .scroll_offset("tree/browser")
                .is_some_and(|offset| offset < bottom)
        );

        let offset = tree
            .state
            .downcast_ref::<State>()
            .engine
            .scroll_offset("tree/browser")
            .unwrap_or_else(|| panic!("the retained offset must survive the upward wheel"));
        let expected = ((offset + 1.0) / builtin::skin().tree.row_height)
            .floor()
            .as_();
        let row_cursor = Cursor::Available(Point::new(area.x + 20.0, area.y + 1.0));
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            row_cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        drop(shell);
        assert_eq!(
            messages.last(),
            Some(&UiEvent::Control {
                path: "tree/browser".to_owned(),
                action: ControlAction::SelectIndex(expected),
            }),
            "row activation must keep the existing SelectIndex emission"
        );
    }

    #[kithara::test]
    fn compiled_tree_surface_installs_the_retained_host() {
        let ui = compiled_tree_surface();
        let reads = FixtureReads::default();
        let element = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let tree = Tree::new(element.as_widget());

        fn retained_hosts(tree: &Tree) -> usize {
            usize::from(tree.tag == Tag::of::<State>())
                + tree.children.iter().map(retained_hosts).sum::<usize>()
        }

        fn retained_state(tree: &Tree) -> Option<&State> {
            if tree.tag == Tag::of::<State>() {
                return Some(tree.state.downcast_ref::<State>());
            }
            tree.children.iter().find_map(retained_state)
        }

        assert_eq!(retained_hosts(&tree), 1);
        assert!(
            retained_state(&tree)
                .and_then(|state| state.engine.scroll_offset("tree/browser"))
                .is_some()
        );
    }

    #[kithara::test]
    fn engine_cursor_wins_and_none_falls_back_to_the_child() {
        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 40.0);
        let cursor = Cursor::Available(Point::new(50.0, 20.0));
        let bounds = Rectangle::with_size(viewport);

        for (layout, expected) in [
            (
                HostedLayout::Control(Some(HostedControl::Activation {
                    path: "hosted/button".to_owned(),
                })),
                mouse::Interaction::Pointer,
            ),
            (
                HostedLayout::Control(None),
                mouse::Interaction::ResizingHorizontally,
            ),
        ] {
            let child = container(
                mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
                    .interaction(mouse::Interaction::ResizingHorizontally),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
            let mut element = Element::new(Host { child, layout });
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &Limits::new(Size::ZERO, viewport),
            );

            assert_eq!(
                element.as_widget().mouse_interaction(
                    &tree,
                    Layout::new(&node),
                    cursor,
                    &bounds,
                    &renderer,
                ),
                expected,
            );
        }
    }

    #[kithara::test]
    fn the_meter_publishes_the_seeked_value_under_its_own_path() {
        let path = "mixer/deck-a/volume";
        let bounds = Rectangle::new(Point::new(0.0, 10.0), Size::new(12.0, 40.0));
        let cursor = Cursor::Available(Point::new(6.0, 30.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let mut engine = Engine::default();
        engine.reconcile([Descriptor::vertical_vu(path.to_owned())]);

        let input = iced_interact::input(&press)
            .unwrap_or_else(|| panic!("a left press must become portable input"));
        let target = Target::new(path, iced_interact::hit(bounds, cursor));
        let emission = engine
            .handle(input, &[target], Instant::now())
            .unwrap_or_else(|| panic!("a press on the meter must publish"));
        let action = engine_event(&emission.path, emission.child, emission.outcome)
            .unwrap_or_else(|| panic!("the published value must cross the iced boundary"));

        assert_eq!(
            action.into_inner().0,
            Some(UiEvent::Control {
                path: path.to_owned(),
                action: ControlAction::SetScalar(0.5),
            })
        );
    }

    #[kithara::test]
    fn gallery_faders_host_their_exact_input_surfaces() {
        let ui = compiled_gallery_faders();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery faders fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-faders-tab");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(components, ["fader", "fader", "vertical-vu"]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 1, "the faders page owns one engine");

        let renderer = headless_renderer();
        let viewport = Size::new(320.0, 500.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            ["faders/default", "faders/volume", "faders/vertical"]
        );
        let [
            Descriptor::Fader {
                drag_step: default_step,
                ..
            },
            Descriptor::Fader {
                drag_step: volume_step,
                ..
            },
            Descriptor::VerticalVu { .. },
        ] = descriptors.as_slice()
        else {
            panic!("the hosted fader descriptors must keep one shared kind");
        };
        assert_eq!(*default_step, Some(builtin::skin().fader.step));
        assert_eq!(*volume_step, None);

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            ["faders/default", "faders/volume", "faders/vertical"]
        );
        let area = |path: &str| {
            targets
                .iter()
                .find(|target| target.path == path)
                .unwrap_or_else(|| panic!("the hosted `{path}` target must exist"))
                .hit
                .area()
        };
        let default = area("faders/default");
        let volume = area("faders/volume");
        let vertical = area("faders/vertical");
        assert_eq!(default.h, 16.0);
        assert_eq!(volume.h, 14.0);
        assert_eq!((vertical.w, vertical.h), (18.0, 120.0));

        let speaker = Cursor::Available(Point::new(
            volume.x - builtin::skin().fader.content_gap - builtin::skin().fader.icon_width / 2.0,
            volume.y + volume.h / 2.0,
        ));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            speaker,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);
        assert!(
            messages.is_empty(),
            "the Volume speaker is outside the fader input surface"
        );

        let cursor = Cursor::Available(Point::new(
            default.x + default.w / 2.0,
            default.y + default.h / 2.0,
        ));
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "faders/default".to_owned(),
                action: ControlAction::SetScalar(0.5),
            }]
        );
    }

    #[kithara::test]
    fn gallery_meters_is_explicitly_hosted_and_routes_the_stereo_gesture() {
        let ui = compiled_gallery_meters();
        let reads = FixtureReads::default();
        let CompiledNode::Module { instance, root, .. } = &ui.root else {
            panic!("gallery fixture root must be a module");
        };
        let ExpandedNode::Column { children, .. } = root.as_ref() else {
            panic!("gallery atoms root must be a column");
        };
        let meters = &children[1];

        assert!(ui.includes_module(*instance, &[1], "gallery-meters"));
        let mut components = Vec::new();
        claimed_components(meters, &mut components);
        assert_eq!(components, ["stereo-meter", "vertical-vu", "vertical-vu"]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "only the meter include owns an engine"
        );

        let renderer = headless_renderer();
        let viewport = Size::new(160.0, 180.0);
        let child = super::super::node::render_engine_node(
            meters,
            &[1],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, meters, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(meters, &ui, &reads, builtin::skin());
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            [
                "atoms/meters/stereo",
                "atoms/meters/vertical-120",
                "atoms/meters/vertical-64",
            ]
        );
        let stereo = targets
            .iter()
            .find(|target| target.path == "atoms/meters/stereo")
            .unwrap_or_else(|| panic!("the hosted stereo meter target must exist"));
        let area = stereo.hit.area();
        assert_eq!((area.w, area.h), (64.0, 22.0));
        let expected_path = stereo.path.to_owned();
        let cursor = Cursor::Available(Point::new(area.x + area.w * 0.25, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);

        assert_eq!(messages.len(), 1);
        let UiEvent::Control { path, action } = &messages[0] else {
            panic!("the hosted stereo meter must publish a control event");
        };
        assert_eq!(path, &expected_path);
        assert_eq!(action, &ControlAction::SetScalar(0.25));
    }

    #[kithara::test]
    fn gallery_toggles_route_activation_without_retaining_capture() {
        let ui = compiled_gallery_toggles();
        let reads = FixtureReads::default();
        let CompiledNode::Module { instance, root, .. } = &ui.root else {
            panic!("gallery fixture root must be a module");
        };
        let ExpandedNode::Column { children, .. } = root.as_ref() else {
            panic!("gallery atoms root must be a column");
        };
        let toggles = &children[1];

        assert!(ui.includes_module(*instance, &[1], "gallery-toggles"));
        let mut components = Vec::new();
        claimed_components(toggles, &mut components);
        assert_eq!(components, ["activation"; 4]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "only the toggles include owns an engine"
        );

        let renderer = headless_renderer();
        let viewport = Size::new(200.0, 100.0);
        let child = super::super::node::render_engine_node(
            toggles,
            &[1],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, toggles, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(toggles, &ui, &reads, builtin::skin());
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();

        for expected_path in ["atoms/toggles/toggle-on", "atoms/toggles/checkbox-on"] {
            let target = targets
                .iter()
                .find(|target| target.path == expected_path)
                .unwrap_or_else(|| panic!("the hosted `{expected_path}` target must exist"));
            let area = target.hit.area();
            let cursor =
                Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
                Layout::new(&node),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
                &Rectangle::with_size(viewport),
            );
            assert!(
                !shell.is_event_captured(),
                "activation publishes without retaining the engine capture slot"
            );
        }

        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "atoms/toggles/toggle-on".to_owned(),
                    action: ControlAction::Activate,
                },
                UiEvent::Control {
                    path: "atoms/toggles/checkbox-on".to_owned(),
                    action: ControlAction::Activate,
                },
            ]
        );
    }

    #[kithara::test]
    fn gallery_chips_host_their_exact_activation_inventory() {
        let ui = compiled_gallery_chips();
        let reads = FixtureReads::default();
        let CompiledNode::Module { instance, root, .. } = &ui.root else {
            panic!("gallery fixture root must be a module");
        };
        let ExpandedNode::Column { children, .. } = root.as_ref() else {
            panic!("gallery atoms root must be a column");
        };
        let chips = &children[1];

        assert!(ui.includes_module(*instance, &[1], "gallery-chips"));
        let mut components = Vec::new();
        claimed_components(chips, &mut components);
        assert_eq!(components, ["activation"; 2]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "only the chips include owns an engine"
        );

        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 80.0);
        let child = super::super::node::render_engine_node(
            chips,
            &[1],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, chips, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(chips, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            ["atoms/chips/active", "atoms/chips/inactive"]
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| matches!(descriptor, Descriptor::Activation { .. }))
        );

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            ["atoms/chips/active", "atoms/chips/inactive"]
        );
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        for expected_path in ["atoms/chips/active", "atoms/chips/inactive"] {
            let target = targets
                .iter()
                .find(|target| target.path == expected_path)
                .unwrap_or_else(|| panic!("the hosted `{expected_path}` target must exist"));
            let area = target.hit.area();
            let cursor =
                Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
                Layout::new(&node),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
                &Rectangle::with_size(viewport),
            );
            assert!(
                !shell.is_event_captured(),
                "activation publishes without retaining the engine capture slot"
            );
        }

        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "atoms/chips/active".to_owned(),
                    action: ControlAction::Activate,
                },
                UiEvent::Control {
                    path: "atoms/chips/inactive".to_owned(),
                    action: ControlAction::Activate,
                },
            ]
        );
    }

    #[kithara::test]
    fn gallery_buttons_share_the_host_activation_component() {
        let ui = compiled_gallery_buttons();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery buttons fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-buttons-tab");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(components, ["activation"; 6]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "the buttons page owns one engine"
        );

        let renderer = headless_renderer();
        let viewport = Size::new(320.0, 160.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            [
                "buttons/play",
                "buttons/cue",
                "buttons/sync",
                "buttons/default",
                "buttons/primary",
                "buttons/micro",
            ]
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| matches!(descriptor, Descriptor::Activation { .. }))
        );

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let center = |path: &str| {
            let target = targets
                .iter()
                .find(|target| target.path == path)
                .unwrap_or_else(|| panic!("the hosted `{path}` target must exist"));
            let area = target.hit.area();
            Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0)
        };
        let cue = center("buttons/cue");
        let state = tree.state.downcast_mut::<State>();
        state.last_hovered_control = Some("buttons/play".to_owned());
        state.last_mouse_interaction = Some(mouse::Interaction::Pointer);
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: cue }),
            Layout::new(&node),
            Cursor::Available(cue),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert_eq!(
            shell.redraw_request(),
            window::RedrawRequest::NextFrame,
            "moving between adjacent activation controls must repaint both hover states"
        );
        drop(shell);

        for expected_path in ["buttons/play", "buttons/default"] {
            let target = targets
                .iter()
                .find(|target| target.path == expected_path)
                .unwrap_or_else(|| panic!("the hosted `{expected_path}` target must exist"));
            let area = target.hit.area();
            let cursor =
                Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
            let mut shell = Shell::new(&mut messages);
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
                Layout::new(&node),
                cursor,
                &renderer,
                &mut clipboard,
                &mut shell,
                &Rectangle::with_size(viewport),
            );
            assert!(
                !shell.is_event_captured(),
                "the hosted `{expected_path}` press in {area:?} publishes without retaining the \
                 engine capture slot"
            );
        }

        assert_eq!(
            messages,
            [
                UiEvent::Control {
                    path: "buttons/play".to_owned(),
                    action: ControlAction::Activate,
                },
                UiEvent::Control {
                    path: "buttons/default".to_owned(),
                    action: ControlAction::Activate,
                },
            ],
            "a button with no read endpoint must still have the same activation contract"
        );
    }

    #[kithara::test]
    fn gallery_cells_hosts_its_exact_engine_control_inventory() {
        let ui = compiled_gallery_cells();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery cells fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-cells-tab");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(
            components,
            [
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "segmented",
                "activation",
                "activation",
                "activation",
                "activation",
            ]
        );

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 1, "the cells page owns one engine");

        let renderer = headless_renderer();
        let viewport = Size::new(1_000.0, 400.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            [
                "cells/cue",
                "cells/play",
                "cells/deck-b",
                "cells/deck-a",
                "cells/fx-1",
                "cells/fx-2",
                "cells/beat",
                "cells/toggle-off",
                "cells/toggle-on",
                "cells/checkbox-off",
                "cells/checkbox-on",
            ]
        );
        assert!(matches!(
            descriptors.as_slice(),
            [
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Segmented { item_count: 4, .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
            ]
        ));

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let target = targets
            .iter()
            .find(|target| target.path == "cells/beat")
            .unwrap_or_else(|| panic!("the hosted segmented target must exist"));
        let area = target.hit.area();
        assert_eq!((area.w, area.h), (220.0, 26.0));
        let cursor = Cursor::Available(Point::new(area.x + area.w * 0.625, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "cells/beat".to_owned(),
                action: ControlAction::SelectIndex(2),
            }]
        );
    }

    #[kithara::test]
    fn gallery_track_list_hosts_its_exact_conditional_inventory() {
        let ui = compiled_gallery_track_list();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery track-list fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-tracklist-tab");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(
            components,
            [
                "segmented",
                "track-list",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
                "activation",
            ]
        );

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "the track-list page owns one engine"
        );

        let renderer = headless_renderer();
        let narrow = Size::new(1_000.0, 640.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node =
            element
                .as_widget_mut()
                .layout(&mut tree, &renderer, &Limits::new(Size::ZERO, narrow));
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets
                .iter()
                .filter(|target| target.path.starts_with("tracklist/table"))
                .map(|target| target.path)
                .collect::<Vec<_>>(),
            [
                "tracklist/table/scroll-x",
                "tracklist/table",
                "tracklist/table/rows",
                "tracklist/table/width/index",
                "tracklist/table/width/deck",
                "tracklist/table/width/artist",
                "tracklist/table/width/bpm",
                "tracklist/table/width/key",
                "tracklist/table/width/time",
            ]
        );
        let descriptors = active_descriptors(&hosted, &targets);
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            [
                "tracklist/column-preset",
                "tracklist/table/scroll-x",
                "tracklist/table",
                "tracklist/table/rows",
                "tracklist/table/width/index",
                "tracklist/table/width/deck",
                "tracklist/table/width/artist",
                "tracklist/table/width/bpm",
                "tracklist/table/width/key",
                "tracklist/table/width/time",
                "tracklist/table/width/energy",
                "tracklist/column-index",
                "tracklist/column-deck",
                "tracklist/column-title",
                "tracklist/column-artist",
                "tracklist/column-bpm",
                "tracklist/column-key",
                "tracklist/column-time",
                "tracklist/column-energy",
                "tracklist/column-transition",
                "tracklist/reset-columns",
            ]
        );
        assert!(matches!(
            descriptors.as_slice(),
            [
                Descriptor::Segmented { item_count: 3, .. },
                Descriptor::Scroll { config: horizontal, .. },
                Descriptor::Scroll { config: vertical, .. },
                Descriptor::Item { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::ColumnDivider { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
                Descriptor::Activation { .. },
            ] if horizontal.axis() == ScrollAxis::Horizontal
                && vertical.axis() == ScrollAxis::Vertical
        ));
        let divider_targets: Vec<_> = targets
            .iter()
            .filter(|target| target.path.contains("/width/"))
            .collect();
        assert_eq!(divider_targets.len(), 6);
        assert!(
            divider_targets.iter().all(|target| {
                target.hit.area().w == builtin::skin().track_list.divider_hit_width
            })
        );
        let viewport = targets
            .iter()
            .find(|target| target.path == "tracklist/table/scroll-x")
            .map_or_else(
                || panic!("the narrow table must expose its horizontal viewport"),
                |target| target.hit.area(),
            );
        assert!(divider_targets.iter().all(|target| {
            let hit = target.hit.area();
            hit.x >= viewport.x && hit.x + hit.w <= viewport.x + viewport.w
        }));

        let wide = Size::new(1_200.0, 640.0);
        let mut child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut child_tree = Tree::new(child.as_widget());
        let wide_node = child.as_widget_mut().layout(
            &mut child_tree,
            &renderer,
            &Limits::new(Size::ZERO, wide),
        );
        let wide_targets = hosted.targets(Layout::new(&wide_node), Cursor::Unavailable);
        assert_eq!(
            wide_targets
                .iter()
                .filter(|target| target.path.starts_with("tracklist/table"))
                .map(|target| target.path)
                .collect::<Vec<_>>(),
            [
                "tracklist/table",
                "tracklist/table/rows",
                "tracklist/table/width/index",
                "tracklist/table/width/deck",
                "tracklist/table/width/artist",
                "tracklist/table/width/bpm",
                "tracklist/table/width/key",
                "tracklist/table/width/time",
                "tracklist/table/width/energy",
            ]
        );
        assert!(
            wide_targets
                .iter()
                .all(|target| target.path != "tracklist/table/scroll-x")
        );
        assert!(
            active_descriptors(&hosted, &wide_targets)
                .iter()
                .all(|descriptor| descriptor_path(descriptor) != "tracklist/table/scroll-x")
        );
    }

    #[kithara::test]
    fn gallery_module_tabs_share_the_host_activation_component() {
        let ui = compiled_gallery_tabs();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery module tabs fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-module-tabs");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(components, ["activation"; 5]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 1, "the tabs own one engine");

        let renderer = headless_renderer();
        let viewport = Size::new(500.0, 80.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            [
                "modules-tabs/deck",
                "modules-tabs/deck-micro",
                "modules-tabs/global-bar",
                "modules-tabs/telemetry",
                "modules-tabs/layout",
            ]
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| matches!(descriptor, Descriptor::Activation { .. }))
        );

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let target = targets
            .iter()
            .find(|target| target.path == "modules-tabs/deck-micro")
            .unwrap_or_else(|| panic!("the hosted DECK MICRO target must exist"));
        let area = target.hit.area();
        assert!((area.w - 94.0).abs() < 0.001);
        assert_eq!(area.h, builtin::skin().tab_large.height);
        let cursor = Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "modules-tabs/deck-micro".to_owned(),
                action: ControlAction::Activate,
            }]
        );
    }

    #[kithara::test]
    fn gallery_nav_shares_the_host_activation_component() {
        let ui = compiled_gallery_nav();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("gallery nav fixture root must be a module");
        };

        assert_eq!(ui.resolve(*module), "gallery-nav");
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(components, ["activation"; 18]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 1, "the nav owns one engine");

        let renderer = headless_renderer();
        let viewport = Size::new(198.0, 620.0);
        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            [
                "gallery/atoms/item",
                "gallery/buttons/item",
                "gallery/faders/item",
                "gallery/modules/item",
                "gallery/typography/item",
                "gallery/cells/item",
                "gallery/sizes/item",
                "gallery/tokens/item",
                "gallery/micro/item",
                "gallery/mixer/item",
                "gallery/vis/item",
                "gallery/chrome/item",
                "gallery/titlebars/item",
                "gallery/tracklist/item",
                "gallery/tree/item",
                "gallery/library2/item",
                "gallery/stress/item",
                "gallery/menu/item",
            ]
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| matches!(descriptor, Descriptor::Activation { .. }))
        );

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let target = targets
            .iter()
            .find(|target| target.path == "gallery/buttons/item")
            .unwrap_or_else(|| panic!("the hosted buttons nav item target must exist"));
        let area = target.hit.area();
        assert_eq!((area.w, area.h), (198.0, builtin::skin().nav.item_height));
        let cursor = Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(
            !shell.is_event_captured(),
            "activation publishes without retaining the engine capture slot"
        );
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "gallery/buttons/item".to_owned(),
                action: ControlAction::Activate,
            }]
        );
    }

    #[kithara::test]
    fn studio_overview_row_hosts_its_click_wave() {
        let ui = compiled_overview_row();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("overview fixture root must be a module");
        };
        let ExpandedNode::Row { children, .. } = root.as_ref() else {
            panic!("overview fixture body must be a row");
        };
        let row = &children[0];

        assert_eq!(ui.resolve(*module), "studio-overview");
        assert!(ui.includes_module(*instance, &[0], "studio-overview-row"));
        let mut components = Vec::new();
        claimed_components(row, &mut components);
        assert_eq!(components, ["wave"]);

        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(
            host_count(&full_tree),
            1,
            "the overview row owns one engine"
        );

        let renderer = headless_renderer();
        let viewport = Size::new(200.0, 40.0);
        let child = super::super::node::render_engine_node(
            row,
            &[0],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, row, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(row, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        assert_eq!(descriptors.len(), 1);
        let Descriptor::Wave { path } = &descriptors[0] else {
            panic!("the overview wave must produce a wave descriptor");
        };
        assert_eq!(path, "overview/a/wave");

        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            ["overview/a/wave"]
        );
        let area = targets[0].hit.area();
        let cursor = Cursor::Available(Point::new(area.x + area.w * 0.25, area.y + area.h / 2.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(
            !shell.is_event_captured(),
            "the click wave publishes without retaining the engine capture slot"
        );
        drop(shell);

        let outside = Point::new(area.x + area.w + 10.0, area.y + area.h / 2.0);
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: outside }),
            Layout::new(&node),
            Cursor::Available(outside),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(!shell.is_event_captured());
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: "overview/a/wave".to_owned(),
                action: ControlAction::SetScalar(0.25),
            }]
        );
    }

    #[kithara::test]
    fn hosted_hero_wave_keeps_grip_outside_bounds() {
        let path = "deck-a/wave";
        let child = container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut element = Element::new(Host {
            child,
            layout: HostedLayout::Control(Some(HostedControl::HeroWave {
                path: path.to_owned(),
                scale: 0.25,
                progress: 0.75,
                visible: 0.625..0.875,
                wheel_positive: 0.3125,
                wheel_non_positive: 0.2,
            })),
        });
        let renderer = headless_renderer();
        let viewport = Size::new(100.0, 40.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            Cursor::Available(Point::new(50.0, 20.0)),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert!(messages.is_empty());

        let outside = Point::new(150.0, 20.0);
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: outside }),
            Layout::new(&node),
            Cursor::Available(outside),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);

        assert_eq!(
            messages,
            [UiEvent::Control {
                path: path.to_owned(),
                action: ControlAction::SetScalar(0.5),
            }]
        );
    }

    #[kithara::test]
    fn outer_module_marker_survives_a_root_include_chain() {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "layout.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "chain",
                root: Module(instance: "mixer", source: "mixer.kmodule.ron"))"#,
        );
        resolver.insert(
            "mixer.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "mixer",
                root: Row(children: [
                    Include(id: "strip", source: "studio-strip.kmodule.ron"),
                ]))"#,
        );
        resolver.insert(
            "studio-strip.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "studio-strip",
                root: Include(id: "body", source: "strip-body.kmodule.ron"))"#,
        );
        resolver.insert(
            "strip-body.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "strip-body",
                root: Knob(id: "gain"))"#,
        );
        let ui = compile(
            "layout.klayout.ron",
            &resolver,
            &Registry::default(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("root include chain must compile: {error}"));
        let CompiledNode::Module { instance, .. } = &ui.root else {
            panic!("fixture root must be a module");
        };

        assert!(ui.includes_module(*instance, &[0], "strip-body"));
        assert!(ui.includes_module(*instance, &[0], "studio-strip"));
    }

    #[kithara::test]
    fn the_app_shaped_mixer_owns_one_engine_for_both_strips() {
        let ui = compiled_fixture();
        let reads = FixtureReads::default();
        let CompiledNode::Module {
            instance,
            module,
            root,
            ..
        } = &ui.root
        else {
            panic!("fixture root must be the mixer module");
        };
        let ExpandedNode::Column { children, .. } = root.as_ref() else {
            panic!("mixer root must be a column");
        };
        let ExpandedNode::Row {
            children: strips, ..
        } = &children[0]
        else {
            panic!("mixer strips must be a row");
        };
        assert_eq!(strips.len(), 3, "two strips must surround one divider");

        assert_eq!(ui.resolve(*module), "studio-mixer");
        assert!(ui.includes_module(*instance, &[0, 0], "studio-strip"));
        assert!(ui.includes_module(*instance, &[0, 2], "studio-strip"));
        let mut components = Vec::new();
        claimed_components(root, &mut components);
        assert_eq!(
            components,
            [
                "knob",
                "knob",
                "knob",
                "vertical-vu",
                "knob",
                "knob",
                "knob",
                "vertical-vu",
                "crossfader",
            ]
        );

        let renderer = headless_renderer();
        let viewport = Size::new(224.0, 420.0);
        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 1, "the whole mixer owns one engine");

        let child = super::super::node::render_engine_node(
            root,
            &[],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, root, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted = HostedLayout::new(root, &ui, &reads, builtin::skin());
        let descriptors = hosted.descriptors();
        let targets = hosted.targets(Layout::new(&node), Cursor::Unavailable);
        let expected_paths = [
            "mixer/a/high",
            "mixer/a/mid",
            "mixer/a/low",
            "mixer/a/volume",
            "mixer/b/high",
            "mixer/b/mid",
            "mixer/b/low",
            "mixer/b/volume",
            "mixer/xfade",
        ];
        assert_eq!(
            descriptors.iter().map(descriptor_path).collect::<Vec<_>>(),
            expected_paths,
            "every interactive control needs its own descriptor"
        );
        assert_eq!(
            targets.iter().map(|target| target.path).collect::<Vec<_>>(),
            expected_paths,
        );
        assert!(
            targets
                .iter()
                .all(|target| target.hit.area().w > 0.0 && target.hit.area().h > 0.0),
            "every retained component must resolve to its paint-only canvas bounds",
        );
        for target in targets.iter().filter(|target| target.path != "mixer/xfade") {
            let area = target.hit.area();
            if target.path.ends_with("/volume") {
                assert_eq!(area.w, 38.0, "the VU target must be its declared canvas");
            } else {
                assert_eq!(
                    (area.w, area.h),
                    (28.0, 39.0),
                    "a knob target must be its intrinsic canvas",
                );
            }
        }

        let high_a = targets
            .iter()
            .find(|target| target.path == "mixer/a/high")
            .unwrap_or_else(|| panic!("strip A high target must exist"));
        let high_b = targets
            .iter()
            .find(|target| target.path == "mixer/b/high")
            .unwrap_or_else(|| panic!("strip B high target must exist"));
        let center = |target: &Target<'_>| {
            let area = target.hit.area();
            Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0)
        };
        let start = center(high_a);
        let over_b = center(high_b);
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            }),
            Layout::new(&node),
            Cursor::Available(start),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(
            !shell.is_event_captured(),
            "a knob wheel emission does not retain the engine capture slot"
        );
        drop(shell);
        assert_eq!(
            messages.len(),
            1,
            "a wheel on the real hosted knob must publish exactly once"
        );
        let UiEvent::Control { path, action } = &messages[0] else {
            panic!("the hosted knob wheel must publish a control event");
        };
        assert_eq!(path, "mixer/a/high");
        assert!(matches!(action, ControlAction::SetScalar(_)));
        messages.clear();

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            Cursor::Available(start),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert!(messages.is_empty(), "arming a knob must not publish");

        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: over_b }),
            Layout::new(&node),
            Cursor::Available(over_b),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert_eq!(
            messages.len(),
            1,
            "the hosted knob's paint-only child must not answer the move a second time, and strip \
             B must stay silent while strip A owns the mixer's capture slot"
        );
        let UiEvent::Control { path, action } = &messages[0] else {
            panic!("the captured strip A knob must publish a control event");
        };
        assert_eq!(path, "mixer/a/high");
        assert_eq!(action, &ControlAction::SetScalar(0.5));
    }

    #[kithara::test]
    fn the_host_retains_an_armed_component_across_fresh_descriptors() {
        let ui = compiled_fixture();
        let reads = FixtureReads::default();
        let CompiledNode::Module { instance, root, .. } = &ui.root else {
            panic!("fixture root must be the mixer module");
        };
        let ExpandedNode::Column { children, .. } = root.as_ref() else {
            panic!("mixer root must be a column");
        };
        let ExpandedNode::Row {
            children: strips, ..
        } = &children[0]
        else {
            panic!("mixer strips must be a row");
        };
        let strip = &strips[0];
        let renderer = headless_renderer();
        let viewport = Size::new(112.0, 420.0);
        let child = super::super::node::render_engine_node(
            strip,
            &[0, 0],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, strip, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let layout = HostedLayout::new(strip, &ui, &reads, builtin::skin());
        let high = layout
            .targets(Layout::new(&node), Cursor::Unavailable)
            .into_iter()
            .find(|target| target.path == "mixer/a/high")
            .unwrap_or_else(|| panic!("strip A high target must exist"));
        let area = high.hit.area();
        let start = Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0);
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Layout::new(&node),
            Cursor::Available(start),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);
        assert!(messages.is_empty(), "arming a knob must not publish");
        drop(element);

        let refreshed_reads = FixtureReads {
            gain: 0.9,
            ..FixtureReads::default()
        };
        let next_child = super::super::node::render_engine_node(
            strip,
            &[0, 0],
            *instance,
            &ui,
            &refreshed_reads,
            builtin::skin(),
        );
        let mut next = host(next_child, strip, &ui, &refreshed_reads, builtin::skin());
        tree.diff(next.as_widget());
        let next_node =
            next.as_widget_mut()
                .layout(&mut tree, &renderer, &Limits::new(Size::ZERO, viewport));
        let moved = Point::new(start.x, start.y - builtin::skin().knob.drag_range * 0.25);
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        next.as_widget_mut().update(
            &mut tree,
            &Event::Mouse(mouse::Event::CursorMoved { position: moved }),
            Layout::new(&next_node),
            Cursor::Available(moved),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(viewport),
        );
        assert!(shell.is_event_captured());
        drop(shell);

        assert_eq!(messages.len(), 1);
        let UiEvent::Control { path, action } = &messages[0] else {
            panic!("the retained knob must publish a control event");
        };
        assert_eq!(path, "mixer/a/high");
        assert_eq!(action, &ControlAction::SetScalar(0.75));
    }
}
