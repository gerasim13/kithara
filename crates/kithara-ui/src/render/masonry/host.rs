use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

use super::{
    Click, CustomWidget, MasonryControl, MasonryKnob, MasonryNode,
    custom::{HostAction, MappedCustom, MountedCustom},
    flex::{ChildLayout, Flex},
    layout::NodeLayout,
    leaf::{DragProgram, Leaf, WindowLeafLayer},
    node::LayerParts,
    popover::{PopoverLayer, PopoverState},
    root::WindowLayer,
};
use crate::{
    atoms::{
        button::{Button, ButtonConfig, ButtonLabel},
        chip::Chip,
        nav_item::NavItem,
    },
    compile::CompiledUi,
    expand::{Binding, ControlSpec, ExpandedNode},
    ids::InternId,
    interact::recognizers::{Track, WheelStep},
    layout::Axis,
    module::{ButtonStyle, ChromeStyle, TextAlign, TextStyle},
    render::{
        ControlAction, HostedControlPlan, InputOwner, ReadValue, Reads, Skin, UiEvent,
        controls::{
            EffectiveIcon, effective_icon, nav_item_supports_engine_input, supports_engine_input,
        },
        document::{Group, Host, Module, Popover, read::resolve},
        hosted_control_plan,
        icons::document_icon,
    },
    size::{Dim, SizeSpec, control_size},
    skin::{ColorRole, TextRoleSkin},
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
                let chip = Click::new(
                    Chip::new(*style, self.reads_true(read), self.skin),
                    self.ui.resolve(*label).to_owned(),
                    self.skin,
                );
                Self::control_leaf(self.owned(chip, owner, path, Click::interactive), declared)
            }
            ControlSpec::NavItem { icon, label } => {
                let value = read.and_then(|binding| resolve(self.reads, binding, self.ui));
                match (document_icon(*icon).lucide_glyph(), value) {
                    (Some(glyph), Some(ReadValue::Bool(active))) => {
                        let item = Click::new(
                            NavItem::new(glyph, active, self.skin),
                            self.ui.resolve(*label).to_owned(),
                            self.skin,
                        );
                        Self::control_leaf(
                            self.owned(item, owner, path, Click::interactive),
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
                let active = self.reads_true(read);
                let glyph = match effective_icon(*style, icon.map(document_icon), active) {
                    EffectiveIcon::Glyph(glyph) => Some(glyph),
                    EffectiveIcon::None => None,
                    EffectiveIcon::Svg(_) => return Self::empty(declared),
                };
                let button = Click::new(
                    Button::new(
                        ButtonConfig::builder()
                            .active(active)
                            .maybe_frame(*frame)
                            .maybe_glyph(glyph)
                            .style(*style)
                            .build(),
                        self.skin,
                    ),
                    ButtonLabel {
                        active: active_label.map(|label| self.ui.resolve(label).to_owned()),
                        label: self.ui.resolve(*label).to_owned(),
                    },
                    self.skin,
                );
                Self::control_leaf(
                    self.owned(button, owner, path, Click::interactive),
                    declared,
                )
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
            let (content, declared, layers, popovers, engine_targets, pickers, window) =
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
            output.append_pickers(pickers);
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

const fn main_length(dim: Dim) -> solve::Length {
    match dim {
        Dim::Fixed(value) => solve::Length::Fixed(value),
        Dim::Range { .. } | Dim::Fill | Dim::Shrink => solve::Length::Fill,
    }
}

const fn length(dim: Dim) -> solve::Length {
    match dim {
        Dim::Fixed(value) => solve::Length::Fixed(value),
        Dim::Shrink => solve::Length::Shrink,
        Dim::Range { .. } | Dim::Fill => solve::Length::Fill,
    }
}

const fn declared(size: SizeSpec) -> solve::Size<solve::Length> {
    solve::Size::new(length(size.w), length(size.h))
}

fn control_declared(
    spec: &ControlSpec,
    size: Option<SizeSpec>,
    skin: &Skin,
) -> solve::Size<solve::Length> {
    let intrinsic = match spec {
        ControlSpec::DeckSummary { .. } => solve::Size::new(
            solve::Length::FillPortion(skin.deck.summary_fill),
            solve::Length::Fixed(skin.deck.summary_height),
        ),
        ControlSpec::Button { style, .. } => solve::Size::new(
            match style {
                ButtonStyle::Transport => solve::Length::FillPortion(skin.button.transport_fill),
                ButtonStyle::TransportPrimary => {
                    solve::Length::FillPortion(skin.button.primary_fill)
                }
                ButtonStyle::Default | ButtonStyle::MicroPrimary | ButtonStyle::VisNav => {
                    solve::Length::Shrink
                }
            },
            solve::Length::Fill,
        ),
        ControlSpec::Text { .. } => solve::Size::new(solve::Length::Shrink, solve::Length::Fill),
        ControlSpec::Spacer | ControlSpec::WindowDrag | ControlSpec::TitleBar { .. } => {
            solve::Size::new(solve::Length::Fill, solve::Length::Fill)
        }
        _ => declared(control_size(spec, skin.document())),
    };
    size.map_or(intrinsic, |size| {
        solve::Size::new(
            control_length(size.w, intrinsic.width),
            control_length(size.h, intrinsic.height),
        )
    })
}

const fn control_length(dim: Dim, intrinsic: solve::Length) -> solve::Length {
    match dim {
        Dim::Fixed(value) => solve::Length::Fixed(value),
        Dim::Shrink => solve::Length::Shrink,
        Dim::Range { .. } => match intrinsic {
            solve::Length::FillPortion(portion) => solve::Length::FillPortion(portion),
            solve::Length::Fill | solve::Length::Shrink | solve::Length::Fixed(_) => {
                solve::Length::Fill
            }
        },
        Dim::Fill => solve::Length::Fill,
    }
}

const fn alignment(value: TextAlign) -> solve::Alignment {
    match value {
        TextAlign::Start => solve::Alignment::Start,
        TextAlign::Center => solve::Alignment::Center,
        TextAlign::End => solve::Alignment::End,
    }
}

fn text_role(
    style: TextStyle,
    color: Option<ColorRole>,
    active_color: Option<ColorRole>,
    active: bool,
    skin: &Skin,
) -> TextRoleSkin {
    let (role, skin_active) = match style {
        TextStyle::Body => (skin.text.body, None),
        TextStyle::Brand => (skin.text.brand, None),
        TextStyle::BrandSmall => (skin.text.brand_small, None),
        TextStyle::DeckLetter => (skin.text.deck_letter, Some(skin.text.deck_letter_active)),
        TextStyle::TrackTitle => (skin.text.track_title, None),
        TextStyle::Telemetry => (skin.text.telemetry, None),
        TextStyle::MicroLabel => (skin.text.micro_label, None),
        TextStyle::Section => (skin.text.section, None),
        TextStyle::Mono => (skin.text.mono, None),
        TextStyle::Caption => (skin.text.caption, None),
        TextStyle::VisFooter | TextStyle::VisMeta => (skin.vis.meta, None),
        TextStyle::VisTitle => (skin.vis.title, None),
    };
    TextRoleSkin {
        color: active
            .then_some(active_color.or(skin_active))
            .flatten()
            .or(color)
            .unwrap_or(role.color),
        ..role
    }
}

/// Whether this control reaches Vello as a painted leaf that can own the
/// pointer itself, rather than as an empty box the engine drives.
fn leaf_paints(spec: &ControlSpec) -> bool {
    match spec {
        ControlSpec::Button { icon, style, .. } => {
            supports_engine_input(*style, icon.map(document_icon))
        }
        ControlSpec::NavItem { icon, .. } => nav_item_supports_engine_input(document_icon(*icon)),
        ControlSpec::Chip { .. } | ControlSpec::Knob { .. } => true,
        _ => false,
    }
}

const fn activates(spec: &ControlSpec) -> bool {
    matches!(
        spec,
        ControlSpec::NavItem { .. }
            | ControlSpec::TabLarge { .. }
            | ControlSpec::Button { .. }
            | ControlSpec::Toggle
            | ControlSpec::Checkbox
            | ControlSpec::Chip { .. }
    )
}
