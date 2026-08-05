use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

use num_traits::cast::AsPrimitive;

use super::{
    CustomWidget, MasonryControl, MasonryKnob, MasonryNode, Painted,
    controls::Retained,
    custom::{HostAction, MappedCustom, MountedCustom},
    flex::{ChildLayout, Flex},
    layout::{
        NodeLayout, activates, alignment, control_declared, declared, leaf_paints, length,
        main_length, text_role,
    },
    leaf::{DragProgram, Leaf, WindowLeafLayer},
    node::LayerParts,
    popover::{PopoverLayer, PopoverState},
    root::WindowLayer,
};
use crate::{
    atoms::{
        button::{Button, ButtonConfig, ButtonLabel},
        chip::Chip,
        design::{
            cell::Cell as CellFace, crossfader::Crossfader, fader::Fader, meter::Meter,
            status_dot::StatusDot, swatch::Swatch,
        },
        meter::StereoMeter,
        nav_item::NavItem,
        painter::{ButtonData, CellData, FaderData, Labelled},
        tab::TabLarge,
        toggle::Binary,
        vu::VerticalVu,
    },
    compile::CompiledUi,
    expand::{Binding, ControlSpec, ExpandedNode},
    ids::InternId,
    interact::recognizers::{Track, WheelStep},
    layout::Axis,
    module::{ChromeStyle, TextStyle},
    render::{
        ControlAction, HostedControlPlan, InputOwner, ReadValue, Reads, Skin, StereoLevels,
        UiEvent,
        controls::button_glyphs,
        document::{Group, Host, Module, Popover, read::resolve},
        hosted_control_plan,
        icons::document_icon,
    },
    size::SizeSpec,
    skin::ColorRole,
    solve,
    text::TextContext,
    widgets::window::{ControlsProgram, TitleProgram},
};

/// Mounts the toolkit-neutral document fold into a retained Masonry widget tree.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
pub struct MasonryHost<'a, Action = UiEvent> {
    ui: &'a CompiledUi,
    reads: &'a dyn Reads,
    skin: &'a Skin,
    custom: BTreeMap<String, Box<dyn MountedCustom>>,
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    #[field(with)]
    state: MasonryState,
    action: std::marker::PhantomData<fn() -> Action>,
}

/// Retained host state shared across Masonry document rebuilds.
#[derive(Clone, Default)]
pub struct MasonryState {
    popovers: Rc<RefCell<BTreeMap<String, Rc<PopoverState>>>>,
    pointer: Rc<Cell<Option<crate::draw::Pt>>>,
}

impl MasonryState {
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
}

impl<'a> MasonryHost<'a, UiEvent> {
    #[must_use]
    pub fn new(ui: &'a CompiledUi, reads: &'a dyn Reads, skin: &'a Skin) -> Self {
        Self::map_actions(ui, reads, skin, |event| event)
    }
}

impl<'a, Action> MasonryHost<'a, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    /// Creates a host that maps built-in document events into one app action.
    #[must_use]
    pub fn map_actions<Map>(
        ui: &'a CompiledUi,
        reads: &'a dyn Reads,
        skin: &'a Skin,
        map: Map,
    ) -> Self
    where
        Map: Fn(UiEvent) -> Action + 'static,
    {
        Self {
            ui,
            reads,
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
    fn empty(declared: solve::Size<solve::Length>) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Empty),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }

    fn text_leaf(
        &self,
        content: String,
        style: TextStyle,
        color: Option<ColorRole>,
        active_color: Option<ColorRole>,
        active: bool,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        let role = text_role(style, color, active_color, active, self.skin);
        let padding_x = match style {
            TextStyle::VisFooter => self.skin.vis.footer_padding_x,
            TextStyle::VisMeta => self.skin.vis.index_padding_x,
            TextStyle::VisTitle => self.skin.vis.name_padding_x,
            _ => 0.0,
        };
        let content = if style == TextStyle::MicroLabel {
            content.to_uppercase()
        } else {
            content
        };
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Text {
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

    fn custom_leaf(
        &self,
        widget: Box<dyn MountedCustom>,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Custom {
                widget,
                text: Box::new(TextContext::from(self.skin.text_resources())),
            }),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }

    fn control_leaf(
        control: impl MasonryControl + 'static,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action> {
        MasonryNode::document(
            NodeLayout::Leaf(Leaf::Control(Box::new(control))),
            declared,
            Vec::new(),
            false,
            None,
            None,
        )
    }
}

/// Choosing the painter for one built-in control.
///
/// Its own block: the match grows by an arm per control the retained host
/// learns to draw, and it should not drag the rest of the host over the
/// size gate with it.
impl<Action> MasonryHost<'_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    /// Builds the leaf that paints one built-in control, or an empty box where
    /// this host cannot paint it yet.
    fn painted_leaf(
        &self,
        spec: &ControlSpec,
        read: Option<&Binding>,
        owner: InputOwner,
        path: &str,
        declared: solve::Size<solve::Length>,
        plan: Option<&HostedControlPlan>,
    ) -> MasonryNode<Action> {
        match spec {
            ControlSpec::Text {
                style,
                label,
                color,
                active_color,
                active,
                ..
            } => {
                let content = read
                    .and_then(|binding| resolve(self.reads, binding, self.ui))
                    .and_then(|value| match value {
                        ReadValue::Text(value) => Some(value.to_owned()),
                        _ => None,
                    })
                    .or_else(|| label.map(|label| self.ui.resolve(label).to_owned()))
                    .unwrap_or_default();
                self.text_leaf(
                    content,
                    *style,
                    *color,
                    *active_color,
                    self.reads_true(active.as_ref()),
                    declared,
                )
            }
            ControlSpec::Knob { label } => match plan {
                Some(HostedControlPlan::Knob {
                    current,
                    drag_range,
                    wheel_step,
                    ..
                }) => {
                    let knob = MasonryKnob::new(
                        label.map(|label| self.ui.resolve(label).to_owned()),
                        *current,
                        self.skin,
                    );
                    let knob = match owner {
                        InputOwner::Leaf => knob.interactive(
                            path.to_owned(),
                            Track::RelativeVertical {
                                range: *drag_range,
                                value: *current,
                            },
                            WheelStep {
                                value: *current,
                                step: *wheel_step,
                            },
                            Rc::clone(&self.map_event),
                        ),
                        InputOwner::Engine => knob,
                    };
                    Self::control_leaf(knob, declared)
                }
                _ => Self::empty(declared),
            },
            ControlSpec::Chip { style, label } => {
                let chip = Painted::new(
                    Chip::new(*style, self.skin),
                    Labelled {
                        active: self.reads_true(read),
                        label: self.ui.resolve(*label).to_owned(),
                    },
                    self.skin,
                );
                Self::control_leaf(
                    self.owned(chip, owner, path, Painted::interactive),
                    declared,
                )
            }
            ControlSpec::TabLarge { label } => {
                let tab = Painted::new(
                    TabLarge::new(self.skin),
                    Labelled {
                        active: self.reads_true(read),
                        label: self.ui.resolve(*label).to_owned(),
                    },
                    self.skin,
                );
                Self::control_leaf(self.owned(tab, owner, path, Painted::interactive), declared)
            }
            // A switch draws nothing until its endpoint says which way it is
            // set, so an unbound one is an empty box rather than an idle switch.
            // An unbound meter is an empty track rather than an empty box:
            // that is what the other host has always drawn for it.
            ControlSpec::Meter => Self::control_leaf(
                Painted::new(
                    Meter::new(self.skin),
                    match read.and_then(|binding| resolve(self.reads, binding, self.ui)) {
                        Some(ReadValue::Scalar(level)) => level.clamp(0.0, 1.0).as_(),
                        _ => 0.0,
                    },
                    self.skin,
                ),
                declared,
            ),
            ControlSpec::Cell { label, highlighted } => Self::control_leaf(
                Painted::new(
                    CellFace::new(self.skin),
                    CellData {
                        highlighted: *highlighted,
                        label: label.map(|label| self.ui.resolve(label).to_owned()),
                    },
                    self.skin,
                ),
                declared,
            ),
            ControlSpec::Swatch { role, label } => Self::control_leaf(
                Painted::new(
                    Swatch::new(*role, self.skin),
                    self.ui.resolve(*label).to_owned(),
                    self.skin,
                ),
                declared,
            ),
            ControlSpec::StatusDot { label, tone } => Self::control_leaf(
                Painted::new(
                    StatusDot::new(*tone, self.skin),
                    self.ui.resolve(*label).to_owned(),
                    self.skin,
                ),
                declared,
            ),
            ControlSpec::Toggle | ControlSpec::Checkbox => {
                let painter = if matches!(spec, ControlSpec::Toggle) {
                    Binary::toggle(self.skin)
                } else {
                    Binary::checkbox(self.skin)
                };
                match read.and_then(|binding| resolve(self.reads, binding, self.ui)) {
                    Some(ReadValue::Bool(active)) => Self::control_leaf(
                        self.owned(
                            Painted::new(painter, active, self.skin),
                            owner,
                            path,
                            Painted::interactive,
                        ),
                        declared,
                    ),
                    _ => Self::empty(declared),
                }
            }
            ControlSpec::VuVertical { ticks } => {
                self.meter_leaf(VerticalVu::new(*ticks, self.skin), read, declared)
            }
            ControlSpec::VuStereo => self.meter_leaf(StereoMeter::new(self.skin), read, declared),
            ControlSpec::Crossfader { ticks } => {
                self.scalar_leaf(Crossfader::new(*ticks, self.skin), read, declared)
            }
            ControlSpec::Fader { style, label } => {
                match read.and_then(|binding| resolve(self.reads, binding, self.ui)) {
                    Some(ReadValue::Scalar(value)) => Self::control_leaf(
                        Painted::new(
                            Fader::new(*style, self.skin),
                            FaderData {
                                label: label.map(|label| self.ui.resolve(label).to_owned()),
                                value: value.clamp(0.0, 1.0).as_(),
                            },
                            self.skin,
                        ),
                        declared,
                    ),
                    _ => Self::empty(declared),
                }
            }
            ControlSpec::NavItem { icon, label } => {
                let value = read.and_then(|binding| resolve(self.reads, binding, self.ui));
                match (document_icon(*icon).lucide_glyph(), value) {
                    (Some(glyph), Some(ReadValue::Bool(active))) => {
                        let item = Painted::new(
                            NavItem::new(glyph, self.skin),
                            Labelled {
                                active,
                                label: self.ui.resolve(*label).to_owned(),
                            },
                            self.skin,
                        );
                        Self::control_leaf(
                            self.owned(item, owner, path, Painted::interactive),
                            declared,
                        )
                    }
                    _ => Self::empty(declared),
                }
            }
            ControlSpec::Button {
                active_label,
                frame,
                icon,
                label,
                style,
            } => {
                let Ok(glyphs) = button_glyphs(*style, icon.map(document_icon)) else {
                    return Self::empty(declared);
                };
                let button = Painted::new(
                    Button::new(
                        ButtonConfig::builder()
                            .maybe_frame(*frame)
                            .maybe_glyph(glyphs.idle)
                            .style(*style)
                            .build(),
                        glyphs.active,
                        self.skin,
                    ),
                    ButtonData {
                        active: self.reads_true(read),
                        label: ButtonLabel {
                            active: active_label.map(|label| self.ui.resolve(label).to_owned()),
                            label: self.ui.resolve(*label).to_owned(),
                        },
                    },
                    self.skin,
                );
                Self::control_leaf(
                    self.owned(button, owner, path, Painted::interactive),
                    declared,
                )
            }
            _ => Self::empty(declared),
        }
    }

    /// Mounts a control the engine drags, painted from the scalar it reads.
    fn scalar_leaf<Painter>(
        &self,
        painter: Painter,
        read: Option<&Binding>,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action>
    where
        Painter: Retained<Data = f32> + 'static,
    {
        match read.and_then(|binding| resolve(self.reads, binding, self.ui)) {
            Some(ReadValue::Scalar(value)) => Self::control_leaf(
                Painted::new(painter, value.clamp(0.0, 1.0).as_(), self.skin),
                declared,
            ),
            _ => Self::empty(declared),
        }
    }

    /// Mounts a meter, which paints the levels it reads and leaves its drag to
    /// the engine plan.
    fn meter_leaf<Painter>(
        &self,
        painter: Painter,
        read: Option<&Binding>,
        declared: solve::Size<solve::Length>,
    ) -> MasonryNode<Action>
    where
        Painter: Retained<Data = StereoLevels> + 'static,
    {
        match read.and_then(|binding| resolve(self.reads, binding, self.ui)) {
            Some(ReadValue::Stereo(levels)) => {
                Self::control_leaf(Painted::new(painter, levels, self.skin), declared)
            }
            _ => Self::empty(declared),
        }
    }

    fn reads_true(&self, read: Option<&Binding>) -> bool {
        read.and_then(|binding| resolve(self.reads, binding, self.ui))
            .is_some_and(|value| matches!(value, ReadValue::Bool(true)))
    }

    /// Gives a control its own click gesture only where the document says the
    /// leaf owns input; an engine-owned control is painted and left alone.
    fn owned<Control>(
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

    fn event(&self, event: impl Fn() -> UiEvent + 'static) -> Box<dyn Fn() -> HostAction> {
        let map = Rc::clone(&self.map_event);
        Box::new(move || map(event()))
    }

    fn control_action(&self, path: String, action: ControlAction) -> Box<dyn Fn() -> HostAction> {
        self.event(move || crate::render::control_event(&path, action.clone()))
    }

    fn shared_control_action(
        &self,
        path: String,
        action: ControlAction,
    ) -> Rc<dyn Fn() -> HostAction> {
        let map = Rc::clone(&self.map_event);
        Rc::new(move || map(crate::render::control_event(&path, action.clone())))
    }

    fn add_window_layer<Program>(&self, output: &mut MasonryNode<Action>, program: Program)
    where
        Program: crate::render::WindowLayerProgram + 'static,
    {
        let geometry = output.track_geometry();
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
}

impl<Action> Host for MasonryHost<'_, Action>
where
    Action: std::fmt::Debug + Send + 'static,
{
    type Output = MasonryNode<Action>;

    fn split(&mut self, axis: Axis, children: Vec<(f32, SizeSpec, Self::Output)>) -> Self::Output {
        let mut layouts = Vec::with_capacity(children.len());
        let mut nodes = Vec::with_capacity(children.len());
        for (weight, size, child) in children {
            let split_size = match axis {
                Axis::Horizontal => solve::Size::new(main_length(size.w), solve::Length::Fill),
                Axis::Vertical => solve::Size::new(solve::Length::Fill, main_length(size.h)),
            };
            layouts.push(ChildLayout::weighted(child.declared(), split_size, weight));
            nodes.push(child);
        }
        MasonryNode::document(
            NodeLayout::Flex(Flex::new(
                axis,
                solve::Length::Fill,
                solve::Length::Fill,
                solve::Padding::default(),
                0.0,
                solve::Alignment::Start,
                layouts,
            )),
            solve::Size::new(solve::Length::Fill, solve::Length::Fill),
            nodes,
            true,
            None,
            None,
        )
    }

    fn module(&mut self, module: Module<'_>, content: Option<Self::Output>) -> Self::Output {
        let chrome = module.chrome();
        let frame = Some((
            module.frame(),
            self.skin.rgba(self.skin.chrome.frame.border),
            self.skin.chrome.frame.border_width,
        ));
        let panel = Some(self.skin.rgba(self.skin.chrome.panel_background));
        match chrome {
            ChromeStyle::Full if module.collapsed() => MasonryNode::document(
                NodeLayout::Leaf(Leaf::Empty),
                solve::Size::new(
                    solve::Length::Fill,
                    solve::Length::Fixed(self.skin.chrome.header_height),
                ),
                Vec::new(),
                false,
                Some(self.skin.rgba(self.skin.chrome.header_background)),
                frame,
            ),
            ChromeStyle::Full => {
                let header = furniture(
                    self.skin.chrome.header_height,
                    Some(self.skin.rgba(self.skin.chrome.header_background)),
                );
                let first_line = furniture(
                    self.skin.chrome.inner_line_width,
                    Some(self.skin.rgba(self.skin.chrome.inner_line)),
                );
                let content = content.unwrap_or_else(|| Self::empty(declared(SizeSpec::FILL)));
                let second_line = furniture(
                    self.skin.chrome.inner_line_width,
                    Some(self.skin.rgba(self.skin.chrome.inner_line)),
                );
                let footer = furniture(
                    self.skin.chrome.footer_height,
                    Some(self.skin.rgba(self.skin.chrome.footer_background)),
                );
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
                let child = content.unwrap_or_else(|| Self::empty(declared(SizeSpec::FILL)));
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

    fn group(
        &mut self,
        group: Group<'_>,
        children: Vec<(Option<f32>, Self::Output)>,
    ) -> Self::Output {
        let size = group.size().unwrap_or(SizeSpec::FILL);
        let mut layouts = Vec::with_capacity(children.len());
        let mut nodes = Vec::with_capacity(children.len());
        for (minimum, child) in children {
            layouts.push(ChildLayout::natural(child.declared(), minimum));
            nodes.push(child);
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
        MasonryNode::document(
            NodeLayout::Flex(Flex::new(
                group.axis(),
                length(size.w),
                length(size.h),
                solve::Padding {
                    top: group.padding_y(),
                    right: group.padding_x(),
                    bottom: group.padding_y(),
                    left: group.padding_x(),
                },
                group.gap(),
                alignment(group.alignment()),
                layouts,
            )),
            declared(size),
            nodes,
            true,
            background,
            frame,
        )
    }

    fn popover(
        &mut self,
        popover: Popover,
        anchor: Self::Output,
        content: Option<Self::Output>,
    ) -> Self::Output {
        let path = self.ui.resolve(popover.path()).to_owned();
        let state = self.state.popover(&path, popover.is_open());
        let dismiss = self.shared_control_action(path.clone(), ControlAction::Activate);
        let size = popover.size().map_or_else(|| anchor.declared(), declared);
        let mut output =
            MasonryNode::document(NodeLayout::Stack, size, vec![anchor], false, None, None);
        output.add_popover(Rc::clone(&state), Rc::clone(&dismiss));
        if let Some(content) = content {
            let (content, declared, layers, popovers, engine_targets, engines, window, watched) =
                LayerParts::from(content);
            let layer = PopoverLayer::new(
                content,
                declared,
                state,
                popover.at(),
                popover.align(),
                self.skin,
            );
            output.append_layers(layers);
            output.append_popovers(popovers);
            output.append_engine_targets(engine_targets);
            output.append_engines(engines);
            output.append_watched(watched);
            if let Some(window) = window {
                output.set_window_tracker(window);
            }
            output.add_layer(masonry::core::NewWidget::new(layer).erased());
        }
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
        let path = self.ui.resolve(path);
        let mut output =
            MasonryNode::document(NodeLayout::Stack, declared, vec![child], false, None, None);
        output.set_actions(
            Some(self.control_action(path.to_owned(), ControlAction::Activate)),
            Some(self.control_action(path.to_owned(), ControlAction::SecondaryActivate)),
        );
        output
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
            NodeLayout::Flex(Flex::new(
                Axis::Vertical,
                solve::Length::Fill,
                declared.height,
                solve::Padding::default(),
                self.skin.layout.grid_gap,
                solve::Alignment::Start,
                layouts,
            )),
            declared,
            children,
            true,
            None,
            None,
        )
    }

    fn control(
        &mut self,
        path: InternId,
        spec: &ControlSpec,
        read: Option<&Binding>,
        owner: InputOwner,
        size: Option<SizeSpec>,
    ) -> Self::Output {
        let declared = control_declared(spec, size, self.skin);
        let plan = hosted_control_plan(path, spec, read, self.ui, self.reads, self.skin);
        let path = self.ui.resolve(path);
        let leaf_owns_control = owner == InputOwner::Leaf && leaf_paints(spec);
        let custom = self.custom.remove(path);
        let custom_installed = custom.is_some();
        let mut output = custom.map_or_else(
            || self.painted_leaf(spec, read, owner, path, declared, plan.as_ref()),
            |widget| self.custom_leaf(widget, declared),
        );
        if custom_installed {
            return output;
        }
        if let Some(read) = read {
            output.watch(read);
        }
        if leaf_owns_control {
            return output;
        }
        match spec {
            ControlSpec::WindowDrag => self.add_window_layer(&mut output, DragProgram),
            ControlSpec::TitleBar { label } => self.add_window_layer(
                &mut output,
                TitleProgram::new(self.ui.resolve(*label), self.skin),
            ),
            ControlSpec::WindowControls { style } => {
                self.add_window_layer(&mut output, ControlsProgram::new(*style, self.skin));
            }
            _ => {}
        }
        if let Some(plan) = plan {
            output.add_engine_control(plan);
            if owner == InputOwner::Leaf {
                output.host_engine(Rc::clone(&self.map_event));
            }
        } else if activates(spec) {
            output.set_actions(
                Some(self.control_action(path.to_owned(), ControlAction::Activate)),
                None,
            );
        } else if matches!(spec, ControlSpec::SettingsButton) {
            output.set_actions(Some(self.event(|| UiEvent::OpenSettings)), None);
        }
        output
    }

    fn hosted(&mut self, _node: &ExpandedNode, mut child: Self::Output) -> Self::Output {
        child.host_engine(Rc::clone(&self.map_event));
        child
    }

    fn window(
        &mut self,
        mut content: Self::Output,
        dragged: Option<String>,
        resize_edges: bool,
    ) -> Self::Output {
        if dragged.is_none() && !resize_edges {
            return content;
        }
        let pointer = Rc::clone(&self.state.pointer);
        let repaint = dragged.is_some();
        let layer = WindowLayer::new(
            dragged,
            resize_edges,
            Rc::clone(&pointer),
            Rc::clone(&self.map_event),
            self.skin,
        );
        let layer = masonry::core::NewWidget::new(layer);
        let layer_id = layer.id();
        content.add_layer(layer.erased());
        content.set_window_layer(pointer, layer_id, repaint);
        content
    }
}

fn furniture<Action>(height: f32, background: Option<crate::draw::Rgba>) -> MasonryNode<Action> {
    MasonryNode::furniture(
        NodeLayout::Leaf(Leaf::Empty),
        solve::Size::new(solve::Length::Fill, solve::Length::Fixed(height)),
        background,
    )
}
