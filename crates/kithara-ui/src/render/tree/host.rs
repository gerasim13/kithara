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

use super::read::{read_flag, read_scope, resolve, wave_zoom};
use crate::{
    compile::CompiledUi,
    engine::{Descriptor, Engine, Target},
    expand::{ControlSpec, ExpandedNode},
    interact::iced as iced_interact,
    module::WaveStyle,
    render::{
        ReadValue, Reads, Skin, UiEvent, controls::supports_engine_input, engine as engine_event,
        icons::document_icon, model::derived,
    },
    size::{Hidden, is_hidden},
    widgets::wave::zoom_math::{clamp_zoom, window_bounds, zoom_for_wheel},
};

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
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
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
        let answered = if let Some(input) = input {
            let targets = self.layout.targets(layout, cursor);
            if let Some(emission) = state.engine.handle(input, &targets, Instant::now()) {
                if let Some(action) = engine_event(&emission.path, emission.child, emission.outcome)
                {
                    let (message, redraw_request, _) = action.into_inner();
                    shell.request_redraw_at(redraw_request);
                    if let Some(message) = message {
                        shell.publish(message);
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if !answered {
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
        if input.is_some() && state.engine.captures_pointer() {
            shell.capture_event();
        }

        if shell.redraw_request() != window::RedrawRequest::NextFrame {
            let targets = self.layout.targets(layout, cursor);
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

fn interaction(
    engine: &Engine,
    layout_tree: &HostedLayout,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
) -> mouse::Interaction {
    engine.cursor(&layout_tree.targets(layout, cursor)).into()
}

fn hovered_control<'a>(targets: &[Target<'a>]) -> Option<&'a str> {
    targets
        .iter()
        .rev()
        .find(|target| target.hit.over())
        .map(|target| target.path)
}

enum HostedLayout {
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
    Passive,
}

impl HostedLayout {
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
            } => Self::Control(HostedControl::new(
                ui.resolve(*path),
                spec,
                read.as_ref()
                    .and_then(|binding| resolve(reads, binding, ui)),
                read_scope(read.as_ref(), ui),
                reads,
                ui,
                skin,
            )),
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
            Self::Group { children, .. } | Self::Slot { children, .. } => {
                for child in children {
                    child.append_descriptors(descriptors);
                }
            }
            Self::Control(Some(control)) => descriptors.push(control.into()),
            Self::Control(None) | Self::Passive => {}
        }
    }

    fn targets<'a>(&'a self, layout: Layout<'_>, cursor: mouse::Cursor) -> Vec<Target<'a>> {
        let mut targets = Vec::new();
        self.append_targets(layout, cursor, &mut targets);
        targets
    }

    fn append_targets<'a>(
        &'a self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        targets: &mut Vec<Target<'a>>,
    ) {
        match self {
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
                    child.append_targets(layout, cursor, targets);
                }
            }
            Self::Slot { sized, children } => {
                let Some(layout) = slot_children(layout, *sized) else {
                    return;
                };
                for (child, layout) in children.iter().zip(layout.children()) {
                    child.append_targets(layout, cursor, targets);
                }
            }
            Self::Control(Some(control)) => {
                let Some(layout) = first_child(layout) else {
                    return;
                };
                targets.push(Target::new(
                    control.path(),
                    iced_interact::hit(layout.bounds(), cursor),
                ));
            }
            Self::Control(None) | Self::Passive => {}
        }
    }
}

enum HostedControl {
    Activation {
        path: String,
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
            (ControlSpec::Toggle | ControlSpec::Checkbox, Some(ReadValue::Bool(_))) => {
                Some(Self::Activation {
                    path: path.to_owned(),
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
            | Self::Crossfader { path }
            | Self::Knob { path, .. }
            | Self::StereoMeter { path }
            | Self::VerticalVu { path }
            | Self::Wave { path }
            | Self::HeroWave { path, .. } => path,
        }
    }
}

impl From<&HostedControl> for Descriptor {
    fn from(control: &HostedControl) -> Self {
        match control {
            HostedControl::Activation { path } => Self::activation(path.clone()),
            HostedControl::Crossfader { path } => Self::crossfader(path.clone()),
            HostedControl::Knob {
                path,
                current,
                drag_range,
                wheel_step,
            } => Self::knob(path.clone(), *current, *drag_range, *wheel_step),
            HostedControl::StereoMeter { path } => Self::stereo_meter(path.clone()),
            HostedControl::VerticalVu { path } => Self::vertical_vu(path.clone()),
            HostedControl::Wave { path } => Self::wave(path.clone()),
            HostedControl::HeroWave {
                path,
                scale,
                progress,
                visible,
                wheel_positive,
                wheel_non_positive,
            } => Self::hero_wave(
                path.clone(),
                *scale,
                *progress,
                visible.clone(),
                *wheel_positive,
                *wheel_non_positive,
            ),
        }
    }
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
            ControlAction, StereoLevels, WaveBucket, WaveformView,
            fonts::{FONT_BYTES, SANS},
        },
        source::{MemResolver, UiConfig},
        widgets::{Widget, wheel::WheelSurface},
    };

    const WAVE_BUCKETS: [WaveBucket; 2] = [
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
    ];

    struct Registry {
        boolean: EndpointDesc,
        scalar: EndpointDesc,
        scoped_scalar: EndpointDesc,
        stereo: EndpointDesc,
        text: EndpointDesc,
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
                waveform: EndpointDesc::new(ValueKind::Waveform).with_scope("deck"),
            }
        }
    }

    impl EndpointRegistry for Registry {
        fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
            match (category, id.0.as_str()) {
                (EndpointCategory::Parameter, "gain") => Some(&self.scalar),
                (EndpointCategory::Telemetry, "levels")
                | (EndpointCategory::Model, "mock.levels") => Some(&self.stereo),
                (
                    EndpointCategory::Model,
                    "mock.toggle.on" | "mock.toggle.off" | "mock.checkbox.on" | "mock.checkbox.off"
                    | "mock.button.play" | "mock.button.cue" | "mock.button.sync",
                ) => Some(&self.boolean),
                (
                    EndpointCategory::Model,
                    "gallery.label.meters"
                    | "gallery.label.toggles"
                    | "gallery.label.transport"
                    | "gallery.label.regular",
                ) => Some(&self.text),
                (EndpointCategory::Model, "mock.wave") => Some(&self.waveform),
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
                "gain" => Some(ReadValue::Scalar(self.gain)),
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
                "gallery.label.meters" => Some(ReadValue::Text("VU / STEREO / VERTICAL")),
                "gallery.label.toggles" => Some(ReadValue::Text("TOGGLES / CHECKBOXES")),
                "gallery.label.transport" => Some(ReadValue::Text("TRANSPORT")),
                "gallery.label.regular" => Some(ReadValue::Text("REGULAR")),
                "mock.wave@deck=a" => Some(ReadValue::Waveform(WaveformView {
                    buckets: &WAVE_BUCKETS,
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
                ControlSpec::Toggle | ControlSpec::Checkbox => components.push("activation"),
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
                _ => {}
            },
            ExpandedNode::Popover { .. } | ExpandedNode::Pressable { .. } => {}
        }
    }

    fn descriptor_path(descriptor: &Descriptor) -> &str {
        match descriptor {
            Descriptor::Activation { path }
            | Descriptor::Crossfader { path }
            | Descriptor::Knob { path, .. }
            | Descriptor::StereoMeter { path }
            | Descriptor::VerticalVu { path }
            | Descriptor::Wave { path }
            | Descriptor::HeroWave { path, .. } => path,
        }
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
