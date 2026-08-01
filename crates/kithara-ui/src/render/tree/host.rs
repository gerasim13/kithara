use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, overlay, renderer,
        widget::{self, Operation, Tree},
    },
    event, window,
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use super::read::{read_flag, resolve};
use crate::{
    compile::CompiledUi,
    engine::{Descriptor, Engine, Target},
    expand::{ControlSpec, ExpandedNode},
    interact::iced as iced_interact,
    render::{ReadValue, Reads, Skin, UiEvent, scalar},
    size::{Hidden, is_hidden},
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
    last_mouse_interaction: Option<mouse::Interaction>,
}

impl State {
    fn new(layout: &HostedLayout) -> Self {
        let mut engine = Engine::default();
        engine.reconcile(layout.descriptors());
        Self {
            engine,
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
        if let Some(input) = iced_interact::input(event) {
            let targets = self.layout.targets(layout, cursor);
            if let Some(emission) = state.engine.handle(input, &targets, Instant::now())
                && let Some(action) = scalar(&emission.path, emission.outcome)
            {
                let (message, redraw_request, status) = action.into_inner();
                shell.request_redraw_at(redraw_request);
                if let Some(message) = message {
                    shell.publish(message);
                }
                if status == event::Status::Captured {
                    shell.capture_event();
                }
            }
        } else {
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

        if shell.redraw_request() != window::RedrawRequest::NextFrame {
            let interaction = interaction(&state.engine, &self.layout, layout, cursor);
            if matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
                state.last_mouse_interaction = Some(interaction);
            } else if state
                .last_mouse_interaction
                .is_some_and(|last| last != interaction)
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
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        interaction(
            &tree.state.downcast_ref::<State>().engine,
            &self.layout,
            layout,
            cursor,
        )
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
    Knob {
        path: String,
        current: f32,
        drag_range: f32,
        wheel_step: f32,
    },
    VerticalVu {
        path: String,
    },
}

impl HostedControl {
    fn new(
        path: &str,
        spec: &ControlSpec,
        value: Option<ReadValue<'_>>,
        skin: &Skin,
    ) -> Option<Self> {
        match (spec, value) {
            (ControlSpec::Knob { .. }, Some(ReadValue::Scalar(value))) => Some(Self::Knob {
                path: path.to_owned(),
                current: value.clamp(0.0, 1.0).as_(),
                drag_range: skin.knob.drag_range,
                wheel_step: skin.knob.wheel_step,
            }),
            (ControlSpec::VuVertical { .. }, Some(ReadValue::Stereo(_))) => {
                Some(Self::VerticalVu {
                    path: path.to_owned(),
                })
            }
            _ => None,
        }
    }

    fn path(&self) -> &str {
        match self {
            Self::Knob { path, .. } | Self::VerticalVu { path } => path,
        }
    }
}

impl From<&HostedControl> for Descriptor {
    fn from(control: &HostedControl) -> Self {
        match control {
            HostedControl::Knob {
                path,
                current,
                drag_range,
                wheel_step,
            } => Self::knob(path.clone(), *current, *drag_range, *wheel_step),
            HostedControl::VerticalVu { path } => Self::vertical_vu(path.clone()),
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
            ControlAction, StereoLevels,
            fonts::{FONT_BYTES, SANS},
        },
        source::{MemResolver, UiConfig},
    };

    struct Registry {
        scalar: EndpointDesc,
        stereo: EndpointDesc,
    }

    impl Default for Registry {
        fn default() -> Self {
            Self {
                scalar: EndpointDesc::new(ValueKind::Scalar),
                stereo: EndpointDesc::new(ValueKind::Stereo),
            }
        }
    }

    impl EndpointRegistry for Registry {
        fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
            match (category, id.0.as_str()) {
                (EndpointCategory::Parameter, "gain") => Some(&self.scalar),
                (EndpointCategory::Telemetry, "levels") => Some(&self.stereo),
                _ => None,
            }
        }
    }

    struct FixtureReads {
        gain: f64,
    }

    impl Default for FixtureReads {
        fn default() -> Self {
            Self { gain: 0.5 }
        }
    }

    impl Reads for FixtureReads {
        fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
            match endpoint {
                "gain" => Some(ReadValue::Scalar(self.gain)),
                "levels" => Some(ReadValue::Stereo(StereoLevels {
                    l: 0.4,
                    r: 0.6,
                    volume: 0.5,
                })),
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

    fn hosted_contract(node: &ExpandedNode, components: &mut Vec<&'static str>) -> bool {
        match node {
            ExpandedNode::Row {
                children, surface, ..
            }
            | ExpandedNode::Column {
                children, surface, ..
            } => {
                surface.is_none()
                    && children
                        .iter()
                        .all(|child| hosted_contract(child, components))
            }
            ExpandedNode::Slot { children, .. } => children
                .iter()
                .all(|child| hosted_contract(child, components)),
            ExpandedNode::Optional { child, .. } => hosted_contract(child, components),
            ExpandedNode::Control { spec, .. } => match spec {
                ControlSpec::Knob { .. } => {
                    components.push("knob");
                    true
                }
                ControlSpec::VuVertical { .. } => {
                    components.push("vertical-vu");
                    true
                }
                ControlSpec::Text { .. } => true,
                _ => false,
            },
            ExpandedNode::Popover { .. } | ExpandedNode::Pressable { .. } => false,
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
        let action = scalar(&emission.path, emission.outcome)
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
    fn the_app_shaped_cut_hosts_only_both_studio_strips() {
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
        let strip_a = &strips[0];
        let strip_b = &strips[2];

        assert!(ui.includes_module(*instance, &[0, 0], "studio-strip"));
        assert!(ui.includes_module(*instance, &[0, 2], "studio-strip"));
        assert!(!ui.includes_module(*instance, &[], "studio-strip"));
        assert!(!ui.includes_module(*instance, &[0], "studio-strip"));
        assert!(!ui.includes_module(*instance, &[1], "studio-strip"));
        for strip in [strip_a, strip_b] {
            let mut components = Vec::new();
            assert!(hosted_contract(strip, &mut components));
            assert_eq!(components, ["knob", "knob", "knob", "vertical-vu"]);
        }

        let renderer = headless_renderer();
        let viewport = Size::new(224.0, 420.0);
        let full = super::super::node::render_compiled(&ui.root, &ui, &reads, builtin::skin());
        let full_tree = Tree::new(full.as_widget());
        assert_eq!(host_count(&full_tree), 2, "only both strips own engines");

        let child = super::super::node::render_engine_node(
            strip_a,
            &[0, 0],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element = host(child, strip_a, &ui, &reads, builtin::skin());
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted_a = HostedLayout::new(strip_a, &ui, &reads, builtin::skin());
        let targets_a = hosted_a.targets(Layout::new(&node), Cursor::Unavailable);

        let child_b = super::super::node::render_engine_node(
            strip_b,
            &[0, 2],
            *instance,
            &ui,
            &reads,
            builtin::skin(),
        );
        let mut element_b = host(child_b, strip_b, &ui, &reads, builtin::skin());
        let mut tree_b = Tree::new(element_b.as_widget());
        let node_b = element_b.as_widget_mut().layout(
            &mut tree_b,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let hosted_b = HostedLayout::new(strip_b, &ui, &reads, builtin::skin());
        let targets_b = hosted_b.targets(Layout::new(&node_b), Cursor::Unavailable);
        assert_eq!(
            targets_a
                .iter()
                .map(|target| target.path)
                .collect::<Vec<_>>(),
            [
                "mixer/a/high",
                "mixer/a/mid",
                "mixer/a/low",
                "mixer/a/volume",
            ],
        );
        assert_eq!(
            targets_b
                .iter()
                .map(|target| target.path)
                .collect::<Vec<_>>(),
            [
                "mixer/b/high",
                "mixer/b/mid",
                "mixer/b/low",
                "mixer/b/volume",
            ],
        );
        assert!(
            targets_a
                .iter()
                .chain(&targets_b)
                .all(|target| target.hit.area().w > 0.0 && target.hit.area().h > 0.0),
            "every retained component must resolve to its paint-only canvas bounds",
        );
        for target in targets_a.iter().chain(&targets_b) {
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

        let volume = targets_a
            .iter()
            .find(|target| target.path == "mixer/a/volume")
            .unwrap_or_else(|| panic!("strip A volume target must exist"));
        let area = volume.hit.area();
        let cursor = Cursor::Available(Point::new(area.x + area.w / 2.0, area.y + area.h / 2.0));
        let event = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &event,
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
            "the hosted leaf must publish exactly once"
        );
        let UiEvent::Control { path, action } = &messages[0] else {
            panic!("the hosted VU must publish a control event");
        };
        assert_eq!(path, "mixer/a/volume");
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

        let refreshed_reads = FixtureReads { gain: 0.9 };
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
