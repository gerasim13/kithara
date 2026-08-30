use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

#[cfg(any(test, feature = "capture"))]
use masonry::core::WidgetId;

use super::{
    CustomWidget, MasonryNode,
    built::{BlockState, LayerParts},
    custom::{HostAction, MappedCustom, MountedCustom},
    flex::{ChildLayout, Flex},
    leaf::{Leaf, WindowLeafLayer},
    mount::{
        Cx, NodeControl, NodeLayout, Viewport, activates, alignment, control_declared, declared,
        main_length, pointer_owner,
    },
    node::Detent,
    popover::{PopoverLayer, PopoverState},
    root::WindowLayer,
    shader::ShaderLeaf,
    spot::{Grip, Spot},
    vis::VisLeaf,
};
use crate::{
    draw::{Rgba, Transform},
    expand::{Binding, ControlSpec, ExpandedNode},
    ids::InternId,
    layout::Axis,
    module::{ChromeStyle, MeasureAxis, TextStyle},
    mount,
    render::{
        ControlAction, CustomSkin, DragGhost, HostedControlPlan, InputOwner, ReadValue, Skin,
        UiEvent,
        document::{
            Ctx, Group, GroupMount, Host, Measured, Module, PlacedMount, Popover, SplitMount,
        },
        hosted_control_plan,
        scroll::{Bar, Window},
    },
    shaping::TextContext,
    size::SizeSpec,
    solve,
};

/// Mounts the toolkit-neutral document fold into a retained Masonry widget tree.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
pub struct MasonryHost<'a, Action = UiEvent> {
    pub(super) skin: &'a Skin,
    pub(super) ctx: Ctx<'a, 'a>,
    pub(super) map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    custom: BTreeMap<String, Box<dyn MountedCustom<HostAction>>>,
    #[field(with)]
    state: MasonryState,
    action: std::marker::PhantomData<fn() -> Action>,
}

/// Retained host state shared across Masonry document rebuilds.
#[derive(Clone, Default)]
pub struct MasonryState {
    #[cfg(any(test, feature = "capture"))]
    paths: Rc<RefCell<BTreeMap<String, WidgetId>>>,
    popovers: Rc<RefCell<BTreeMap<String, Rc<PopoverState>>>>,
    windows: Rc<RefCell<BTreeMap<String, Rc<RefCell<Window>>>>>,
    pointer: Rc<Cell<Option<crate::draw::Pt>>>,
}

impl MasonryState {
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn clear_paths(&self) {
        self.paths.borrow_mut().clear();
    }

    fn popover(&self, path: &str, open: bool) -> Rc<PopoverState> {
        let mut popovers = self.popovers.borrow_mut();
        let state = Rc::clone(
            popovers
                .entry(path.to_owned())
                .or_insert_with(|| Rc::new(PopoverState::default())),
        );
        state.latch(open);
        state
    }

    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn tag_path(&self, path: &str, id: WidgetId) {
        self.paths.borrow_mut().insert(path.to_owned(), id);
    }

    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn widget_id(&self, path: &str) -> Option<WidgetId> {
        self.paths.borrow().get(path).copied()
    }

    /// The window one scroll of the document looks through.
    ///
    /// Where a window is scrolled to belongs to the host and not to the
    /// document, and this host builds another tree whenever the application
    /// turns to a different document. A window kept only by the tree would
    /// start over at the top every time that happened, which throws the hand
    /// back to the first row whenever it picks one of the last.
    fn window(&self, path: &str) -> Rc<RefCell<Window>> {
        Rc::clone(
            self.windows
                .borrow_mut()
                .entry(path.to_owned())
                .or_insert_with(|| Rc::new(RefCell::new(Window::new()))),
        )
    }
}

impl<'a> MasonryHost<'a, UiEvent> {
    #[must_use]
    pub fn new(ctx: Ctx<'a, 'a>, skin: &'a Skin) -> Self {
        Self::map_actions(ctx, skin, |event| event)
    }
}

impl<'a, Action> MasonryHost<'a, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    /// Creates a host that maps built-in document events into one app action.
    #[must_use]
    pub fn map_actions<Map>(ctx: Ctx<'a, 'a>, skin: &'a Skin, map: Map) -> Self
    where
        Map: Fn(UiEvent) -> Action + 'static,
    {
        Self {
            ctx,
            skin,
            custom: BTreeMap::new(),
            map_event: Rc::new(move |event| HostAction::new(map(event))),
            state: MasonryState::default(),
            action: std::marker::PhantomData,
        }
    }

    /// Installs custom content at one resolved document control path.
    #[must_use]
    pub fn with_custom<Path, Widget, Map>(mut self, path: Path, widget: Widget, map: Map) -> Self
    where
        Path: Into<String>,
        Widget: CustomWidget,
        Map: Fn(Widget::Action) -> Action + 'static,
    {
        self.custom.insert(
            path.into(),
            Box::new(MappedCustom::new(widget, move |action| {
                HostAction::new(map(action))
            })),
        );
        self
    }
}

/// Mounting one document control as a leaf.
impl<Action> MasonryHost<'_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    /// The state one child of a flow is hidden by, read once for the tree it
    /// is mounted into and kept for the root to read again.
    ///
    /// The child is mounted whether the document hides it or not, so the flow
    /// above it can show it again without anything being rebuilt.
    fn block(
        &self,
        binding: Option<Binding>,
        blocks: &mut Vec<(Binding, Rc<BlockState>)>,
    ) -> Option<Rc<BlockState>> {
        let binding = binding?;
        let state = Rc::new(BlockState::default());
        state.latch(self.ctx.flag(Some(&binding)));
        blocks.push((binding, Rc::clone(&state)));
        Some(state)
    }

    /// A leaf drawing content this toolkit does not own.
    ///
    /// The dressing is taken here rather than at paint because the leaf
    /// outlives the skin it was mounted from: this host rebuilds its tree when
    /// the skin changes, which is the same moment every other leaf takes its
    /// colours at. A widget installed at a path has no kind for the skin to
    /// dress and is dressed in nothing.
    pub(super) fn custom_leaf(
        &self,
        widget: Box<dyn MountedCustom<HostAction>>,
        kind: Option<&str>,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Custom {
                widget,
                skin: kind.map_or_else(CustomSkin::default, |kind| self.skin.custom(kind).clone()),
                text: Box::new(TextContext::from(self.skin.text_resources())),
            }),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }

    /// One module's chrome around the content the walk already produced.
    fn mount_module(
        &self,
        module: &mut Module<'_>,
        content: Option<MasonryNode<Action>>,
    ) -> MasonryNode<Action> {
        let chrome = module.chrome();
        let frame = Some((
            module.frame(),
            self.skin.rgba(self.skin.chrome.frame.border),
            self.skin.chrome.frame.border_width,
        ));
        let panel = Some(self.skin.rgba(self.skin.chrome.panel_background));
        match chrome {
            ChromeStyle::Full if module.collapsed() => MasonryNode::chrome(
                NodeLayout::Stack,
                solve::Size::new(
                    solve::Length::Fill,
                    solve::Length::Fixed(self.skin.chrome.header_height),
                ),
                vec![self.module_header(module)],
                panel,
                frame,
            ),
            ChromeStyle::Full => {
                let header = self.module_header(module);
                let first_line = furniture(
                    self.skin.chrome.inner_line_width,
                    Some(self.skin.rgba(self.skin.chrome.inner_line)),
                );
                let content =
                    content.unwrap_or_else(|| MasonryNode::empty(declared(SizeSpec::FILL)));
                let second_line = furniture(
                    self.skin.chrome.inner_line_width,
                    Some(self.skin.rgba(self.skin.chrome.inner_line)),
                );
                let footer = self.module_footer(module.take_footer().unwrap_or_default());
                let children = vec![header, first_line, content, second_line, footer];
                let layouts = children
                    .iter()
                    .map(|child| ChildLayout::natural(child.declared(), None))
                    .collect();
                MasonryNode::document(
                    NodeLayout::Flex(Flex::new(
                        Axis::Vertical,
                        solve::Length::Fill,
                        solve::Length::Fill,
                        solve::Padding::default(),
                        0.0,
                        solve::Alignment::Start,
                        layouts,
                    )),
                    solve::Size::new(solve::Length::Fill, solve::Length::Fill),
                    children,
                    true,
                    panel,
                    frame,
                )
            }
            ChromeStyle::Frame | ChromeStyle::Plain => {
                let child = content.unwrap_or_else(|| MasonryNode::empty(declared(SizeSpec::FILL)));
                MasonryNode::document(
                    NodeLayout::Stack,
                    solve::Size::new(solve::Length::Fill, solve::Length::Fill),
                    vec![child],
                    true,
                    (chrome == ChromeStyle::Frame).then_some(panel).flatten(),
                    (chrome == ChromeStyle::Frame).then_some(frame).flatten(),
                )
            }
        }
    }

    /// The weighted flow one split lays its cells out in.
    fn mount_split(
        &self,
        axis: Axis,
        measure: Option<MeasureAxis>,
        children: Vec<SplitMount<MasonryNode<Action>>>,
    ) -> MasonryNode<Action> {
        let mut layouts: Vec<ChildLayout> = Vec::with_capacity(children.len());
        let mut nodes: Vec<MasonryNode<Action>> = Vec::with_capacity(children.len());
        let mut blocks = Vec::new();
        for cell in children {
            let split_size = match axis {
                Axis::Horizontal => solve::Size::new(main_length(cell.size.w), solve::Length::Fill),
                Axis::Vertical => solve::Size::new(solve::Length::Fill, main_length(cell.size.h)),
            };
            let block = self.block(cell.block, &mut blocks);
            layouts.push(
                ChildLayout::weighted(cell.output.declared(), split_size, cell.weight)
                    .within(cell.band)
                    .blocked(block),
            );
            nodes.push(cell.output);
        }
        let mut output = MasonryNode::document(
            NodeLayout::Flex(
                Flex::new(
                    axis,
                    solve::Length::Fill,
                    solve::Length::Fill,
                    solve::Padding::default(),
                    0.0,
                    solve::Alignment::Start,
                    layouts,
                )
                .measure(measure),
            ),
            solve::Size::new(solve::Length::Fill, solve::Length::Fill),
            nodes,
            true,
            None,
            None,
        );
        output.hides(blocks);
        output
    }

    pub(super) fn shader_leaf(
        &self,
        spec: crate::shader::ShaderSpec,
        path: String,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Shader(ShaderLeaf::new(spec, path, self.ctx))),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }

    /// The text leaf `spec` describes, carrying `content` and whether the
    /// control reads as active right now. Both are resolved by the caller,
    /// which is the only thing that can reach the readings.
    pub(super) fn text_leaf(
        &self,
        spec: &mount::Text<'_>,
        content: String,
        active: bool,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        let style = spec.style;
        let role = self
            .skin
            .text_role(style, spec.color, spec.active_color, active)
            .faced(spec.font, spec.weight);
        let padding_x = match style {
            TextStyle::VisFooter => self.skin.vis.footer_padding_x,
            TextStyle::VisMeta => self.skin.vis.index_padding_x,
            TextStyle::VisTitle => self.skin.vis.name_padding_x,
            _ => 0.0,
        };
        let content = style.cased(content);
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Text {
                align: spec.align,
                content,
                role,
                padding_x,
                color: self.skin.rgba(role.color),
                text: Box::new(TextContext::from(self.skin.text_resources())),
            }),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }

    pub(super) fn vis_leaf(
        &self,
        preset: Option<String>,
        value: Option<ReadValue<'_>>,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Vis(VisLeaf::new(preset, value, self.ctx))),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }
}

/// Mounting one built-in control, and the pieces its own file reaches for.
///
/// Its own block so the leaf helpers do not drag the rest of the host over
/// the size gate.
impl<Action> MasonryHost<'_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    pub(super) fn add_window_layer<Program>(
        &self,
        output: &mut MasonryNode<Action>,
        program: Program,
    ) where
        Program: crate::render::WindowLayerProgram + 'static,
    {
        let geometry = output.geometry();
        let pointer = Rc::clone(&self.state.pointer);
        let layer = WindowLeafLayer::new(
            program,
            geometry,
            Rc::clone(&pointer),
            Rc::clone(&self.map_event),
        );
        output.add_layer(masonry::core::NewWidget::new(layer).erased());
        output.set_window_pointer(pointer);
    }

    fn control_action(&self, path: String, action: ControlAction) -> Box<dyn Fn() -> HostAction> {
        self.event(move || crate::render::control_event(&path, action.clone()))
    }

    pub(super) fn event(
        &self,
        event: impl Fn() -> UiEvent + 'static,
    ) -> Box<dyn Fn() -> HostAction> {
        let map = Rc::clone(&self.map_event);
        Box::new(move || map(event()))
    }

    /// Gives a control its own click gesture only where the document says the
    /// leaf owns input; an engine-owned control is painted and left alone.
    pub(super) fn owned<Control>(
        &self,
        control: Control,
        owner: InputOwner,
        path: &str,
        interactive: impl FnOnce(Control, String, Rc<dyn Fn(UiEvent) -> HostAction>) -> Control,
    ) -> Control {
        match owner {
            InputOwner::Leaf => interactive(control, path.to_owned(), Rc::clone(&self.map_event)),
            InputOwner::Engine => control,
        }
    }

    pub(super) fn reads_true(&self, read: Option<&Binding>) -> bool {
        read.and_then(|binding| self.ctx.read(binding))
            .is_some_and(|value| matches!(value, ReadValue::Bool(true)))
    }

    fn shared_control_action(
        &self,
        path: String,
        action: ControlAction,
    ) -> Rc<dyn Fn() -> HostAction> {
        let map = Rc::clone(&self.map_event);
        Rc::new(move || map(crate::render::control_event(&path, action.clone())))
    }
}

impl<Action> Host for MasonryHost<'_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    type Output = MasonryNode<Action>;

    /// A retained tree mounts a hidden block and hides it in the flow above
    /// it, because a block left out of the tree could never come back without
    /// the tree being rebuilt around it.
    const MOUNTS_HIDDEN: bool = true;

    fn control(
        &mut self,
        path: InternId,
        spec: &ControlSpec,
        read: Option<&Binding>,
        owner: InputOwner,
        size: Option<SizeSpec>,
        transform: Transform,
    ) -> Self::Output {
        let declared = control_declared(spec, size, self.skin);
        let plan = hosted_control_plan(path, spec, read, self.ctx, self.skin);
        let path_id = path;
        let path = self.ctx.ui.resolve(path);
        let owns_pointer = pointer_owner(owner, spec);
        let custom = self.custom.remove(path);
        let custom_installed = custom.is_some();
        let cx = Cx {
            declared,
            owner: owns_pointer,
            path,
            plan: plan.as_ref(),
            read,
            skin: self.skin.at(path),
        };
        let mut output = custom.map_or_else(
            || {
                mount::controls!(
                    spec,
                    Mount {
                        cx: &cx,
                        host: &*self
                    }
                )
            },
            |widget| self.custom_leaf(widget, None, declared),
        );
        output.place(transform);
        if self.ctx.ui.driven {
            output.watch_placement(path_id);
        }
        #[cfg(any(test, feature = "capture"))]
        self.state.tag_path(path, output.widget_id());
        if custom_installed {
            return output;
        }
        if matches!(
            spec,
            ControlSpec::Vis | ControlSpec::Table { .. } | ControlSpec::Tree { .. }
        ) {
            output.watch_snapshot();
        } else if let Some(read) = read {
            output.watch(read);
        }
        if owns_pointer == InputOwner::Leaf {
            return output;
        }
        mount::controls!(
            spec,
            Wire {
                host: &*self,
                cx: &cx,
                output: &mut output
            }
        );
        if let Some(plan) = plan {
            output.add_engine_control(plan, false);
            if owner == InputOwner::Leaf {
                output.host_engine(Rc::clone(&self.map_event), self.skin);
            }
        } else if activates(spec) {
            output.set_actions(
                Some(self.control_action(path.to_owned(), ControlAction::Activate)),
                None,
            );
        }
        output
    }

    fn group(&mut self, group: Group<'_>, children: Vec<GroupMount<Self::Output>>) -> Self::Output {
        let size = group.size().unwrap_or(SizeSpec::FILL);
        let mut layouts: Vec<ChildLayout> = Vec::with_capacity(children.len());
        let mut nodes: Vec<MasonryNode<Action>> = Vec::with_capacity(children.len());
        let mut blocks = Vec::new();
        for child in children {
            let block = self.block(child.block, &mut blocks);
            layouts.push(
                ChildLayout::natural(child.output.declared(), child.minimum)
                    .within(child.band)
                    .blocked(block),
            );
            nodes.push(child.output);
        }
        let background = group.background().map(|role| {
            let mut color = self.skin.rgba(role);
            color.a = group.background_alpha().unwrap_or(1.0);
            color
        });
        let frame = group.frame().map(|sides| {
            (
                sides,
                self.skin.rgba(group.frame_color()),
                group.frame_width(),
            )
        });
        let mut output = MasonryNode::document(
            NodeLayout::Flex(
                Flex::new(
                    group.axis(),
                    solve::length(size.w),
                    solve::length(size.h),
                    solve::Padding {
                        top: group.padding_y(),
                        right: group.padding_x(),
                        bottom: group.padding_y(),
                        left: group.padding_x(),
                    },
                    group.gap(),
                    alignment(group.alignment()),
                    layouts,
                )
                .measure(group.measure()),
            ),
            declared(size),
            nodes,
            true,
            background,
            frame,
        )
        .rounded(group.round(), self.skin.chrome.frame.radius);
        if let Some(surface) = group.surface() {
            output.set_detent(Detent::new(
                self.ctx.ui.resolve(surface.path).to_owned(),
                Rc::clone(&self.map_event),
            ));
        }
        output.hides(blocks);
        output
    }

    fn hosted(&mut self, _node: &ExpandedNode, mut child: Self::Output) -> Self::Output {
        child.host_engine(Rc::clone(&self.map_event), self.skin);
        child
    }

    fn measured(&mut self, plan: Measured, branches: Vec<Self::Output>) -> Self::Output {
        let size = declared(plan.size);
        MasonryNode::document(NodeLayout::Measured(plan), size, branches, true, None, None)
    }

    fn module(&mut self, mut module: Module<'_>, content: Option<Self::Output>) -> Self::Output {
        let mut output = self
            .mount_module(&mut module, content)
            .rounded(module.round(), self.skin.chrome.frame.radius);
        if module.drop().is_some() {
            let instance = self.ctx.ui.resolve(module.instance());
            output.add_engine_control(HostedControlPlan::crossing(instance), true);
            output.host_engine(Rc::clone(&self.map_event), self.skin);
        }
        output
    }

    /// One placement of a stage: the child keeps the box it asked for, and the
    /// stage lays it out at the point the placement carries. A placement with
    /// somewhere to write also carries the grip that moves that point, and the
    /// endpoint it is re-read from between mounts.
    fn placed(&mut self, placement: PlacedMount<'_>, child: Self::Output) -> Self::Output {
        let declared = child.declared();
        let mut output =
            MasonryNode::document(NodeLayout::Stack, declared, vec![child], false, None, None);
        let grip = placement.write.map(|_| {
            Grip::new(
                self.ctx.ui.resolve(placement.path).to_owned(),
                placement.snap.clone(),
                Rc::clone(&self.map_event),
            )
        });
        output.set_spot(Spot::new(placement.at, grip));
        if let Some(read) = placement.read {
            output.watch_spot(read);
        }
        output
    }

    fn popover(
        &mut self,
        popover: Popover<'_>,
        anchor: Self::Output,
        content: &mut dyn FnMut(&mut Self) -> Self::Output,
    ) -> Self::Output {
        let content = content(self);
        let path = self.ctx.ui.resolve(popover.path()).to_owned();
        let state = self.state.popover(&path, popover.is_open());
        let dismiss = self.shared_control_action(path.clone(), ControlAction::Activate);
        let size = popover.size().map_or_else(|| anchor.declared(), declared);
        let mut output =
            MasonryNode::document(NodeLayout::Stack, size, vec![anchor], false, None, None);
        let (
            content,
            declared,
            layers,
            popovers,
            blocks,
            engine_targets,
            engines,
            boxes,
            native,
            window,
            watched,
        ) = LayerParts::from(content);
        let layer = masonry::core::NewWidget::new(PopoverLayer::new(
            content,
            declared,
            Rc::clone(&state),
            popover.at(),
            popover.align(),
            self.skin,
        ))
        .erased();
        output.add_popover(layer.id(), popover.flag(), state, Rc::clone(&dismiss));
        output.append_layers(layers);
        output.append_popovers(popovers);
        output.append_blocks(blocks);
        output.append_engine_targets(engine_targets);
        output.append_engines(engines);
        output.append_boxes(boxes);
        output.append_native(native);
        output.append_watched(watched);
        if let Some(window) = window {
            output.set_window_tracker(window);
        }
        output.add_layer(layer);
        output.set_actions(
            Some(self.control_action(path, ControlAction::Activate)),
            None,
        );
        output
    }

    fn pressable(
        &mut self,
        path: InternId,
        child: Self::Output,
        size: Option<SizeSpec>,
    ) -> Self::Output {
        let declared = size.map_or_else(|| child.declared(), declared);
        let path = self.ctx.ui.resolve(path);
        let mut output =
            MasonryNode::document(NodeLayout::Stack, declared, vec![child], false, None, None);
        output.add_engine_control(
            HostedControlPlan::Activation {
                path: path.to_owned(),
            },
            true,
        );
        output.host_engine(Rc::clone(&self.map_event), self.skin);
        output.set_actions(
            None,
            Some(self.control_action(path.to_owned(), ControlAction::SecondaryActivate)),
        );
        output
    }

    /// The window is the declared box and the child keeps its own height, so
    /// the retained viewport has something to travel over.
    fn scroll(
        &mut self,
        id: InternId,
        child: Self::Output,
        size: Option<SizeSpec>,
    ) -> Self::Output {
        let declared = size.map_or_else(|| child.declared(), declared);
        let view = self.state.window(self.ctx.ui.resolve(id));
        MasonryNode::document(
            NodeLayout::Scroll(Viewport::new(Bar::new(self.skin), view)),
            declared,
            vec![child],
            false,
            None,
            None,
        )
    }

    fn slot(&mut self, children: Vec<Self::Output>, size: Option<SizeSpec>) -> Self::Output {
        let declared = size.map_or(
            solve::Size::new(solve::Length::Fill, solve::Length::Shrink),
            declared,
        );
        let layouts = children
            .iter()
            .map(|child| ChildLayout::natural(child.declared(), None))
            .collect();
        MasonryNode::document(
            NodeLayout::Flex(
                Flex::new(
                    Axis::Vertical,
                    solve::Length::Fill,
                    declared.height,
                    solve::Padding::default(),
                    self.skin.layout.grid_gap,
                    solve::Alignment::Start,
                    layouts,
                )
                .align_main(solve::Alignment::Center),
            ),
            declared,
            children,
            true,
            None,
            None,
        )
    }

    fn split(
        &mut self,
        axis: Axis,
        measure: Option<MeasureAxis>,
        children: Vec<SplitMount<Self::Output>>,
    ) -> Self::Output {
        self.mount_split(axis, measure, children)
    }

    /// The stack measures its first child and hands every child that box, so
    /// the document's own size rule and the immediate host's `Stack` agree with
    /// it without either of them being told about the other.
    fn stage(&mut self, children: Vec<Self::Output>, size: Option<SizeSpec>) -> Self::Output {
        let declared = size.map_or_else(
            || {
                children
                    .first()
                    .map_or_else(|| declared(SizeSpec::FILL), MasonryNode::declared)
            },
            declared,
        );
        MasonryNode::document(NodeLayout::Stage, declared, children, true, None, None)
    }

    fn window(
        &mut self,
        mut content: Self::Output,
        carried: Option<&Binding>,
        resize_edges: bool,
    ) -> Self::Output {
        if carried.is_none() && !resize_edges {
            return content;
        }
        let label = self.ctx.label(carried);
        let ghost = carried.is_some().then(|| DragGhost::new(label, self.skin));
        let pointer = Rc::clone(&self.state.pointer);
        let layer = WindowLayer::new(
            ghost,
            resize_edges,
            Rc::clone(&pointer),
            Rc::clone(&self.map_event),
            self.skin,
        );
        let layer = masonry::core::NewWidget::new(layer);
        let layer_id = layer.id();
        content.add_layer(layer.erased());
        content.set_window_layer(pointer, layer_id, carried.cloned(), label.is_some());
        content
    }
}

fn furniture<Action>(height: f32, background: Option<Rgba>) -> MasonryNode<Action> {
    MasonryNode::furniture(
        NodeLayout::Leaf(Leaf::Empty),
        solve::Size::new(solve::Length::Fill, solve::Length::Fixed(height)),
        background,
    )
}

/// Asks whichever control the document named to mount itself as a leaf.
struct Mount<'cx, 'host, 'a, Action> {
    cx: &'cx Cx<'cx>,
    host: &'host MasonryHost<'a, Action>,
}

impl<Action> Mount<'_, '_, '_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    fn apply<C: NodeControl>(self, control: &C) -> MasonryNode<Action> {
        control.leaf(self.host, self.cx)
    }
}

/// Asks whichever control the document named to attach whatever it still needs
/// beyond its own leaf.
struct Wire<'out, 'host, 'cx, 'a, Action> {
    cx: &'cx Cx<'cx>,
    host: &'host MasonryHost<'a, Action>,
    output: &'out mut MasonryNode<Action>,
}

impl<Action> Wire<'_, '_, '_, '_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    fn apply<C: NodeControl>(self, control: &C) {
        control.wire(self.host, self.cx, self.output);
    }
}
