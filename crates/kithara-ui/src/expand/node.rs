use std::collections::BTreeMap;

use crate::{
    error::UiDocError,
    ids::{InternId, SourceUri},
    layout::FrameSides,
    module::{
        BindingRef, ButtonStyle, ChipStyle, ChromeStyle, ControlNode, DeckSummaryStyle, FaderStyle,
        GlyphStyle, IconName, MeasureAxis, Motion, PopoverAlign, PopoverAt, Pose, ScalarFormat,
        TableColumn, TextAlign, TextStyle, Tone, WaveStyle, WindowControlsStyle,
    },
    shader::ShaderSpec,
    size::{BlockNode, SizeSpec},
    skin::ColorRole,
};

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ExpandedNode {
    Row {
        id: Option<InternId>,
        size: Option<SizeSpec>,
        measure: Option<MeasureAxis>,
        gap: Option<f32>,
        align: TextAlign,
        pad: Option<f32>,
        pad_x: Option<f32>,
        pad_y: Option<f32>,
        frame: Option<FrameSides>,
        background: Option<ColorRole>,
        background_alpha: Option<f32>,
        active: Option<Binding>,
        active_background: Option<ColorRole>,
        frame_color: Option<ColorRole>,
        active_frame_color: Option<ColorRole>,
        surface: Option<SurfaceSpec>,
        children: Vec<Self>,
    },
    Column {
        id: Option<InternId>,
        size: Option<SizeSpec>,
        measure: Option<MeasureAxis>,
        gap: Option<f32>,
        align: TextAlign,
        pad: Option<f32>,
        pad_x: Option<f32>,
        pad_y: Option<f32>,
        frame: Option<FrameSides>,
        frame_color: Option<ColorRole>,
        background: Option<ColorRole>,
        background_alpha: Option<f32>,
        surface: Option<SurfaceSpec>,
        children: Vec<Self>,
    },
    Scroll {
        id: InternId,
        size: Option<SizeSpec>,
        child: Box<Self>,
    },
    /// Offsets what its subtree draws, and nothing else.
    ///
    /// The pose is resolved per frame rather than at compile time, because the
    /// endpoint behind it may move it between one frame and the next. Layout,
    /// addresses, and pointer regions are the child's alone.
    Object {
        pose: Pose,
        to: Option<Pose>,
        phase: Option<Binding>,
        motion: Option<Motion<Binding>>,
        child: Box<Self>,
    },
    /// Draws one branch: the last step whose threshold the measure reaches,
    /// and `base` below the first of them.
    Adaptive {
        measure: MeasureSpec,
        size: Option<SizeSpec>,
        base: Box<Self>,
        steps: Vec<(f32, Self)>,
    },
    /// Laid out while the enclosing container measures a number in `[from,
    /// until)` on the axis it declares.
    Reveal {
        from: f32,
        until: Option<f32>,
        child: Box<Self>,
    },
    Optional {
        block: BlockSpec,
        child: Box<Self>,
    },
    /// `content` is laid out only inside the overlay, so the node's intrinsic
    /// size is the anchor's alone.
    Popover {
        path: InternId,
        open: Binding,
        at: PopoverAt,
        align: PopoverAlign,
        anchor: Box<Self>,
        content: Box<Self>,
    },
    Pressable {
        path: InternId,
        press: Binding,
        child: Box<Self>,
    },
    /// Offers every child the same box, in document order.
    Stage {
        id: InternId,
        size: Option<SizeSpec>,
        children: Vec<Self>,
    },
    Slot {
        id: InternId,
        size: Option<SizeSpec>,
        children: Vec<Self>,
    },
    Control {
        path: InternId,
        id: InternId,
        spec: ControlSpec,
        size: Option<SizeSpec>,
        read: Option<Binding>,
        write: Option<Binding>,
    },
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ControlSpec {
    DeckSummary {
        style: DeckSummaryStyle,
    },
    Brand,
    Spacer,
    Divider,
    PresetSelector,
    SettingsButton,
    WindowDrag,
    TitleBar {
        label: InternId,
    },
    WindowControls {
        style: WindowControlsStyle,
    },
    Text {
        style: TextStyle,
        label: Option<InternId>,
        color: Option<ColorRole>,
        active_color: Option<ColorRole>,
        active: Option<Binding>,
        align: TextAlign,
    },
    Glyph {
        icon: IconName,
        active_icon: Option<IconName>,
        style: GlyphStyle,
        color: Option<ColorRole>,
        active_color: Option<ColorRole>,
        active: Option<Binding>,
    },
    NavItem {
        label: InternId,
        icon: IconName,
    },
    TabLarge {
        label: InternId,
    },
    Button {
        label: InternId,
        icon: Option<IconName>,
        active_label: Option<InternId>,
        style: ButtonStyle,
        frame: Option<FrameSides>,
    },
    Bpm {
        placeholder: Option<InternId>,
    },
    Time,
    Scalar {
        format: ScalarFormat,
        framed: bool,
    },
    Crossfader {
        ticks: bool,
    },
    Fader {
        style: FaderStyle,
        label: Option<InternId>,
    },
    Wave {
        style: WaveStyle,
        badge: Option<InternId>,
        zoom: Option<Binding>,
    },
    Vis,
    /// One frame of a named artwork, chosen by how far its reading has run and
    /// how long `seconds` says one pass through the artwork takes.
    Lottie {
        artwork: InternId,
        seconds: f32,
    },
    /// One frame of a named sheet, chosen by how far its reading has run and
    /// how long `seconds` says one pass through the sheet takes.
    Sprite {
        sheet: InternId,
        seconds: f32,
    },
    Shader(ShaderSpec),
    PortalMap,
    Range,
    Table {
        columns: Vec<TableColumn>,
        columns_state: Option<Binding>,
    },
    Tree {
        query: Option<Binding>,
    },
    ContextBar {
        scope_items: Vec<InternId>,
        scope: Option<Binding>,
    },
    Toggle,
    Checkbox,
    Segmented {
        items: Vec<InternId>,
    },
    Select {
        label: InternId,
    },
    StatusDot {
        label: InternId,
        dot_size: Option<f32>,
        tone: Tone,
        active_tone: Option<Tone>,
        active: Option<Binding>,
    },
    Swatch {
        role: ColorRole,
        label: InternId,
    },
    Cell {
        label: Option<InternId>,
        highlighted: bool,
    },
    Readout {
        label: Option<InternId>,
        tone: Tone,
        framed: bool,
    },
    Chip {
        label: InternId,
        style: ChipStyle,
    },
    Knob {
        label: Option<InternId>,
    },
    Meter,
    VuStereo,
    VuVertical {
        ticks: bool,
    },
}

/// Which side of the host contract a [`Binding`] addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BindingKind {
    Command,
    Parameter,
    Telemetry,
    Model,
}

/// Compiled endpoint reference. `id` is the bare endpoint; `key` is the
/// canonical scope-qualified form `<id>@<scope>=<value>[,...]` (equal to `id`
/// when the binding has no scope). Renderers and hosts address reads by `key`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Binding {
    pub with: BTreeMap<InternId, InternId>,
    pub kind: BindingKind,
    pub id: InternId,
    pub key: InternId,
}

#[derive(Debug)]
pub(crate) struct ExpandedModule {
    pub(crate) chrome: ChromeStyle,
    pub(crate) root: ExpandedNode,
    pub(crate) collapsed: InternId,
    pub(crate) module: InternId,
    pub(crate) chip: Option<InternId>,
    pub(crate) drop: Option<DropSpec>,
    pub(crate) footer: Option<Binding>,
    pub(crate) title: Option<InternId>,
    pub(crate) assign: Vec<InternId>,
    pub(crate) includes: Vec<ExpandedInclude>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExpandedInclude {
    pub(crate) address: Box<[usize]>,
    pub(crate) module: InternId,
}

/// A block the host may hide: the path that addresses it, and the Bool it
/// reads. While that read is true the block is not laid out.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BlockSpec {
    pub hidden: Binding,
    pub path: InternId,
}

/// Where the number that picks a branch comes from: the box the node is given,
/// or a scalar the host answers.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum MeasureSpec {
    Width,
    Height,
    Read(Binding),
}

impl MeasureSpec {
    pub(crate) const fn axis(&self) -> Option<MeasureAxis> {
        match self {
            Self::Width => Some(MeasureAxis::Width),
            Self::Height => Some(MeasureAxis::Height),
            Self::Read(_) => None,
        }
    }

    pub(crate) const fn binding(&self) -> Option<&Binding> {
        match self {
            Self::Read(binding) => Some(binding),
            Self::Width | Self::Height => None,
        }
    }
}

pub(crate) fn adaptive_branch<'a>(
    base: &'a ExpandedNode,
    steps: &'a [(f32, ExpandedNode)],
    value: Option<f32>,
) -> &'a ExpandedNode {
    let Some(value) = value else {
        return base;
    };
    steps
        .iter()
        .rev()
        .find(|(from, _)| *from <= value)
        .map_or(base, |(_, node)| node)
}

impl BlockNode for ExpandedNode {
    fn block(&self) -> Option<&BlockSpec> {
        match self {
            Self::Optional { block, .. } => Some(block),
            _ => None,
        }
    }
}

impl ControlSpec {
    /// Whether this control draws a new picture every frame of its own accord,
    /// with no endpoint and no input involved.
    ///
    /// A visualisation is a picture of a moment rather than of a value: it keeps
    /// its own decay between frames, so a host that stops drawing it stops it.
    /// Everything else changes only when what it reads changes, and is drawn
    /// again then.
    ///
    /// A shader belongs with everything else, not with the visualisation beside
    /// it. It draws exactly what its uniforms say, and every uniform is an
    /// endpoint — so a shader bound to the host's own clock moves by itself and
    /// is caught as a clock reader, and one bound to endpoints that hold still
    /// draws the same picture however often it is asked.
    ///
    /// Spelled out rather than defaulted, so a control added tomorrow stops
    /// compiling here instead of quietly joining the majority.
    pub(crate) const fn paints_every_frame(&self) -> bool {
        match self {
            Self::Vis => true,
            Self::Shader(_)
            | Self::Bpm { .. }
            | Self::Brand
            | Self::Button { .. }
            | Self::Cell { .. }
            | Self::Checkbox
            | Self::Chip { .. }
            | Self::ContextBar { .. }
            | Self::Crossfader { .. }
            | Self::DeckSummary { .. }
            | Self::Divider
            | Self::Fader { .. }
            | Self::Glyph { .. }
            | Self::Knob { .. }
            | Self::Lottie { .. }
            | Self::Meter
            | Self::NavItem { .. }
            | Self::PortalMap
            | Self::PresetSelector
            | Self::Range
            | Self::Readout { .. }
            | Self::Scalar { .. }
            | Self::Segmented { .. }
            | Self::Select { .. }
            | Self::SettingsButton
            | Self::Spacer
            | Self::Sprite { .. }
            | Self::StatusDot { .. }
            | Self::Swatch { .. }
            | Self::TabLarge { .. }
            | Self::Table { .. }
            | Self::Text { .. }
            | Self::Time
            | Self::TitleBar { .. }
            | Self::Toggle
            | Self::Tree { .. }
            | Self::VuStereo
            | Self::VuVertical { .. }
            | Self::Wave { .. }
            | Self::WindowControls { .. }
            | Self::WindowDrag => false,
        }
    }
}

/// What a subtree does with nothing touching it.
///
/// Both halves are asked for together because they are answered by one walk of
/// the same tree, and because a host that separates them ends up with two
/// accounts of when a document moves.
#[derive(Clone, Copy, Default)]
pub(crate) struct Unprompted {
    /// Something here is placed by an endpoint rather than by the document
    /// alone, so re-reading the endpoints can move a tree already mounted.
    ///
    /// An object needs both ends of a track and something driving it to move at
    /// all: a far pose nobody travels towards, or a phase with nowhere to carry
    /// the object, both leave it exactly where the document wrote it. A host
    /// asks this to do no pose work on a page that cannot move.
    pub(crate) driven: bool,
    /// Something here draws a new picture every frame of its own accord, with
    /// no endpoint and no input involved.
    pub(crate) continuous: bool,
}

impl Unprompted {
    const DRIVEN: Self = Self {
        driven: true,
        continuous: false,
    };

    pub(crate) fn or(self, other: Self) -> Self {
        Self {
            driven: self.driven || other.driven,
            continuous: self.continuous || other.continuous,
        }
    }

    fn of(nodes: &[ExpandedNode]) -> Self {
        nodes.iter().map(motion_of).fold(Self::default(), Self::or)
    }
}

/// What one subtree does with nothing touching it.
pub(crate) fn motion_of(node: &ExpandedNode) -> Unprompted {
    match node {
        ExpandedNode::Object {
            to,
            phase,
            motion,
            child,
            ..
        } => {
            let own = if to.is_some() && (phase.is_some() || motion.is_some()) {
                Unprompted::DRIVEN
            } else {
                Unprompted::default()
            };
            own.or(motion_of(child))
        }
        ExpandedNode::Row { children, .. }
        | ExpandedNode::Column { children, .. }
        | ExpandedNode::Stage { children, .. }
        | ExpandedNode::Slot { children, .. } => Unprompted::of(children),
        ExpandedNode::Popover {
            anchor, content, ..
        } => motion_of(anchor).or(motion_of(content)),
        ExpandedNode::Optional { child, .. }
        | ExpandedNode::Pressable { child, .. }
        | ExpandedNode::Reveal { child, .. }
        | ExpandedNode::Scroll { child, .. } => motion_of(child),
        // Which branch stands is settled by the room, so the node moves if any
        // branch it may draw does.
        ExpandedNode::Adaptive { base, steps, .. } => steps
            .iter()
            .map(|(_, branch)| motion_of(branch))
            .fold(motion_of(base), Unprompted::or),
        ExpandedNode::Control { spec, .. } => Unprompted {
            driven: false,
            continuous: spec.paints_every_frame(),
        },
    }
}

/// Control path a wheel detent publishes on, and the scalar it steps.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SurfaceSpec {
    pub write: Binding,
    pub path: InternId,
}

/// Compiled drop target of a module: the command the host runs when a drag is
/// released over it, and the flag that reads true while one hovers it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DropSpec {
    pub read: Binding,
    pub write: Binding,
}

#[derive(Clone, Copy)]
pub(crate) struct ControlSite<'a> {
    pub(crate) control: &'a ControlNode,
    /// Already resolved, so a parameterised list is validated like a literal one.
    pub(crate) columns: &'a [TableColumn],
    pub(crate) path: &'a str,
    pub(crate) active: Option<&'a BindingRef>,
    pub(crate) columns_state: Option<&'a BindingRef>,
    pub(crate) query: Option<&'a BindingRef>,
    pub(crate) read: Option<&'a BindingRef>,
    pub(crate) scope: Option<&'a BindingRef>,
    pub(crate) write: Option<&'a BindingRef>,
    pub(crate) zoom: Option<&'a BindingRef>,
}

pub(crate) type ControlVisitor<'v> =
    dyn for<'a> FnMut(ControlSite<'a>, &SourceUri) -> Result<(), UiDocError> + 'v;

pub(crate) struct Budget {
    max: usize,
    nodes: usize,
}

impl Budget {
    pub(crate) const fn new(max: usize) -> Self {
        Self { max, nodes: 0 }
    }

    pub(crate) fn charge(&mut self, origin: &SourceUri) -> Result<(), UiDocError> {
        self.nodes += 1;
        if self.nodes > self.max {
            return Err(UiDocError::NodesExceeded {
                origin: origin.clone(),
                count: self.nodes,
                max: self.max,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    fn form(gap: f32) -> ExpandedNode {
        ExpandedNode::Row {
            gap: Some(gap),
            id: None,
            size: None,
            align: TextAlign::default(),
            measure: None,
            pad: None,
            pad_x: None,
            pad_y: None,
            frame: None,
            background: None,
            background_alpha: None,
            active: None,
            active_background: None,
            frame_color: None,
            active_frame_color: None,
            surface: None,
            children: Vec::new(),
        }
    }

    #[kithara::test]
    fn a_measure_takes_the_last_step_it_reaches() {
        let base = form(0.0);
        let steps = vec![(4.0, form(4.0)), (8.0, form(8.0))];
        let branch = |value| adaptive_branch(&base, &steps, value);

        assert_eq!(branch(None), &base, "nothing read");
        assert_eq!(branch(Some(3.9)), &base, "below the first step");
        assert_eq!(branch(Some(4.0)), &steps[0].1, "exactly on a threshold");
        assert_eq!(branch(Some(7.9)), &steps[0].1);
        assert_eq!(branch(Some(8.0)), &steps[1].1);
        assert_eq!(branch(Some(f32::MAX)), &steps[1].1, "above every step");
        assert_eq!(branch(Some(f32::NAN)), &base, "ordered against nothing");
    }
}
