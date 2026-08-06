use std::rc::Rc;

use masonry::{
    core::{BoxConstraints, LayoutCtx, WidgetPod},
    kurbo::{Point, Size as MasonrySize},
};
use num_traits::cast::AsPrimitive;

use super::{
    MasonryHost, MasonryKnob, MasonryNode, Painted,
    controls::Retained,
    flex::{box_constraints, normalized},
    leaf::{DragProgram, Leaf},
    node::Node,
};
use crate::{
    atoms::{
        button::declared_width,
        design::{crossfader::Crossfader, fader::Fader},
        painter::{FaderData, Labelled},
        tab::TabLarge,
    },
    expand::{Binding, ControlSpec},
    interact::recognizers::{Track, WheelStep},
    module::{TextAlign, TextStyle},
    mount,
    render::{
        HostedControlPlan, InputOwner, ReadValue, Skin, UiEvent,
        controls::{Draws, Grip},
        document::read::resolve,
    },
    size::{Dim, SizeSpec, control_size},
    skin::{ColorRole, TextRoleSkin},
    solve,
    widgets::window::{ControlsProgram, TitleProgram},
};

/// How one built-in control becomes a leaf of the retained tree.
///
/// The default is an empty box of the right size: this host paints a control
/// only once its painter is neutral, and until then the control still holds its
/// place. Which controls are still waiting is the census in `tests`, not a
/// silent arm in a match.
pub(super) trait NodeControl {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        host.empty(cx.declared)
    }

    /// Anything the host must still attach once the leaf exists: a window layer
    /// for the controls that move the window, a settings action for the one
    /// that opens it.
    fn wire<A>(&self, host: &MasonryHost<'_, A>, output: &mut MasonryNode<A>)
    where
        A: std::fmt::Debug + Send + 'static,
    {
        let _ = (host, output);
    }
}

/// What a control is handed when it mounts: the box it was given, the endpoint
/// behind it, and the engine plan that may already drive it.
pub(super) struct Cx<'a> {
    pub(super) declared: solve::Size<solve::Length>,
    pub(super) owner: InputOwner,
    pub(super) path: &'a str,
    pub(super) plan: Option<&'a HostedControlPlan>,
    pub(super) read: Option<&'a Binding>,
}

impl NodeControl for mount::Summary {}
impl NodeControl for mount::Brand {}
impl NodeControl for mount::Spacer {}
impl NodeControl for mount::Divider {}
impl NodeControl for mount::Preset {}
impl NodeControl for mount::Settings {
    fn wire<A>(&self, host: &MasonryHost<'_, A>, output: &mut MasonryNode<A>)
    where
        A: std::fmt::Debug + Send + 'static,
    {
        output.set_actions(Some(host.event(|| UiEvent::OpenSettings)), None);
    }
}

impl NodeControl for mount::Drag {
    fn wire<A>(&self, host: &MasonryHost<'_, A>, output: &mut MasonryNode<A>)
    where
        A: std::fmt::Debug + Send + 'static,
    {
        host.add_window_layer(output, DragProgram);
    }
}

impl NodeControl for mount::TitleBar {
    fn wire<A>(&self, host: &MasonryHost<'_, A>, output: &mut MasonryNode<A>)
    where
        A: std::fmt::Debug + Send + 'static,
    {
        host.add_window_layer(
            output,
            TitleProgram::new(host.ui.resolve(self.label), host.skin),
        );
    }
}

impl NodeControl for mount::Controls {
    fn wire<A>(&self, host: &MasonryHost<'_, A>, output: &mut MasonryNode<A>)
    where
        A: std::fmt::Debug + Send + 'static,
    {
        host.add_window_layer(output, ControlsProgram::new(self.style, host.skin));
    }
}

impl NodeControl for mount::Glyph<'_> {}
impl NodeControl for mount::Bpm {}
impl NodeControl for mount::Time {}
impl NodeControl for mount::Telemetry {}
impl NodeControl for mount::Wave<'_> {}
impl NodeControl for mount::Vis {}
impl NodeControl for mount::TrackList<'_> {}
impl NodeControl for mount::Tree<'_> {}
impl NodeControl for mount::ContextBar<'_> {}
impl NodeControl for mount::Segmented<'_> {}
impl NodeControl for mount::Select {}
impl NodeControl for mount::Readout {}

impl NodeControl for mount::Text<'_> {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        let content = cx
            .read
            .and_then(|binding| resolve(host.reads, binding, host.ui))
            .and_then(|value| match value {
                ReadValue::Text(value) => Some(value.to_owned()),
                _ => None,
            })
            .or_else(|| self.label.map(|label| host.ui.resolve(label).to_owned()))
            .unwrap_or_default();
        host.text_leaf(
            content,
            self.style,
            self.color,
            self.active_color,
            host.reads_true(self.active),
            cx.declared,
        )
    }
}

impl NodeControl for mount::Knob {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        let Some(HostedControlPlan::Knob {
            current,
            drag_range,
            wheel_step,
            ..
        }) = cx.plan
        else {
            return host.empty(cx.declared);
        };
        let knob = MasonryKnob::new(
            self.label.map(|label| host.ui.resolve(label).to_owned()),
            *current,
            host.skin,
        );
        let knob = match cx.owner {
            InputOwner::Leaf => knob.interactive(
                cx.path.to_owned(),
                Track::RelativeVertical {
                    range: *drag_range,
                    value: *current,
                },
                WheelStep {
                    value: *current,
                    step: *wheel_step,
                },
                Rc::clone(&host.map_event),
            ),
            InputOwner::Engine => knob,
        };
        host.control_leaf(knob, cx.declared)
    }
}

impl NodeControl for mount::Chip {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Tab {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        let tab = Painted::new(
            TabLarge::new(host.skin),
            Labelled {
                active: host.reads_true(cx.read),
                label: host.ui.resolve(self.label).to_owned(),
            },
            host.skin,
        );
        host.control_leaf(
            host.owned(tab, cx.owner, cx.path, Painted::interactive),
            cx.declared,
        )
    }
}

/// An unbound meter is an empty track rather than an empty box: that is what
/// the other host has always drawn for it.
impl NodeControl for mount::Meter {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Cell {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Swatch {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::StatusDot {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Toggle {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Checkbox {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::VuVertical {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::VuStereo {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Crossfader {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        host.scalar_leaf(Crossfader::new(self.ticks, host.skin), cx.read, cx.declared)
    }
}

impl NodeControl for mount::Fader {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        let Some(ReadValue::Scalar(value)) = cx
            .read
            .and_then(|binding| resolve(host.reads, binding, host.ui))
        else {
            return host.empty(cx.declared);
        };
        host.control_leaf(
            Painted::new(
                Fader::new(self.style, host.skin),
                FaderData {
                    label: self.label.map(|label| host.ui.resolve(label).to_owned()),
                    value: value.clamp(0.0, 1.0).as_(),
                },
                host.skin,
            ),
            cx.declared,
        )
    }
}

impl NodeControl for mount::NavItem {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

impl NodeControl for mount::Button {
    fn leaf<A>(&self, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
    where
        A: std::fmt::Debug + Send + 'static,
    {
        painted(self, host, cx)
    }
}

pub(crate) enum NodeLayout {
    Leaf(Leaf),
    Flex(super::flex::Flex),
    Stack,
}

impl NodeLayout {
    pub(crate) fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        children: &mut [WidgetPod<Node>],
        limits: solve::Limits,
        declared: solve::Size<solve::Length>,
    ) -> solve::Size {
        match self {
            Self::Leaf(leaf) => {
                let intrinsic = leaf.measure(limits);
                limits.resolve(declared.width, declared.height, intrinsic)
            }
            Self::Flex(flex) => {
                let intrinsic = flex.layout(ctx, children, limits);
                limits.resolve(declared.width, declared.height, intrinsic)
            }
            Self::Stack => stack(ctx, children, limits, declared),
        }
    }

    pub(crate) const fn leaf(&mut self) -> Option<&mut Leaf> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Flex(_) | Self::Stack => None,
        }
    }

    pub(crate) fn accepts_input(&self) -> bool {
        matches!(self, Self::Leaf(leaf) if leaf.accepts_input())
    }

    pub(crate) fn accepts_text_input(&self) -> bool {
        matches!(self, Self::Leaf(leaf) if leaf.accepts_text_input())
    }
}

fn stack(
    ctx: &mut LayoutCtx<'_>,
    children: &mut [WidgetPod<Node>],
    limits: solve::Limits,
    declared: solve::Size<solve::Length>,
) -> solve::Size {
    let inner = normalized(limits.width(declared.width).height(declared.height).loose());
    let intrinsic = children.first_mut().map_or(solve::Size::ZERO, |first| {
        Node::set_child_limits(ctx, first, inner);
        let size = ctx.run_layout(first, &box_constraints(inner));
        solve::Size::new(size.width.as_(), size.height.as_())
    });
    let size = limits.resolve(declared.width, declared.height, intrinsic);
    let exact = solve::Limits::new(size, size);
    for child in children {
        Node::set_child_limits(ctx, child, exact);
        ctx.run_layout(
            child,
            &BoxConstraints::tight(MasonrySize::new(
                f64::from(size.width),
                f64::from(size.height),
            )),
        );
        ctx.place_child(child, Point::ORIGIN);
    }
    size
}

pub(crate) const fn main_length(dim: Dim) -> solve::Length {
    match dim {
        Dim::Fixed(value) => solve::Length::Fixed(value),
        Dim::Range { .. } | Dim::Fill | Dim::Shrink => solve::Length::Fill,
    }
}

pub(crate) const fn length(dim: Dim) -> solve::Length {
    match dim {
        Dim::Fixed(value) => solve::Length::Fixed(value),
        Dim::Shrink => solve::Length::Shrink,
        Dim::Range { .. } | Dim::Fill => solve::Length::Fill,
    }
}

pub(crate) const fn declared(size: SizeSpec) -> solve::Size<solve::Length> {
    solve::Size::new(length(size.w), length(size.h))
}

pub(crate) fn control_declared(
    spec: &ControlSpec,
    size: Option<SizeSpec>,
    skin: &Skin,
) -> solve::Size<solve::Length> {
    let intrinsic = match spec {
        ControlSpec::DeckSummary { .. } => solve::Size::new(
            solve::Length::FillPortion(skin.deck.summary_fill),
            solve::Length::Fixed(skin.deck.summary_height),
        ),
        ControlSpec::Button { style, .. } => {
            solve::Size::new(declared_width(*style, skin), solve::Length::Fill)
        }
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

pub(crate) const fn control_length(dim: Dim, intrinsic: solve::Length) -> solve::Length {
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

pub(crate) const fn alignment(value: TextAlign) -> solve::Alignment {
    match value {
        TextAlign::Start => solve::Alignment::Start,
        TextAlign::Center => solve::Alignment::Center,
        TextAlign::End => solve::Alignment::End,
    }
}

pub(crate) fn text_role(
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
pub(crate) fn leaf_paints(spec: &ControlSpec) -> bool {
    match spec {
        ControlSpec::Button { .. }
        | ControlSpec::Chip { .. }
        | ControlSpec::Knob { .. }
        | ControlSpec::NavItem { .. }
        | ControlSpec::TabLarge { .. } => true,
        _ => false,
    }
}

pub(crate) const fn activates(spec: &ControlSpec) -> bool {
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

/// Mounts a control that draws itself, adding nothing to the picture.
fn painted<Control, A>(control: &Control, host: &MasonryHost<'_, A>, cx: &Cx<'_>) -> MasonryNode<A>
where
    Control: Draws,
    Control::Painter: Retained + 'static,
    A: std::fmt::Debug + Send + 'static,
{
    let value = cx
        .read
        .and_then(|binding| resolve(host.reads, binding, host.ui));
    let Some(data) = control.data(value.as_ref(), host.ui) else {
        return host.empty(cx.declared);
    };
    let leaf = Painted::new(control.painter(host.skin), data, host.skin);
    let leaf = match control.grip() {
        Grip::Press => host.owned(leaf, cx.owner, cx.path, Painted::interactive),
        // A scalar drag is a gesture only the immediate host recognises; here
        // the engine plan drives it, which is what this host has always done.
        // The two are reconciled by the gesture census, not by this arm.
        Grip::Drag { .. } | Grip::None => leaf,
    };
    host.control_leaf(leaf, cx.declared)
}
