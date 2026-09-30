use super::binding::BindingSide;
use crate::{
    expand::ControlSite,
    interact::recognizers::Edge,
    module::{BindingRef, ControlNode},
    registry::ValueKind,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Gesture {
    Press,
    Secondary,
    Scalar,
    Step,
    Index,
    Text,
    Place,
}

#[derive(Clone, Copy)]
pub(crate) struct WriteSlot<'a> {
    pub(crate) binding: &'a BindingRef,
    pub(crate) child: Option<&'static str>,
    pub(crate) edge: Option<Edge>,
    pub(crate) gesture: Gesture,
    pub(crate) kind: ValueKind,
    pub(in crate::validate) side: BindingSide,
}

impl<'a> WriteSlot<'a> {
    const fn new(binding: &'a BindingRef, gesture: Gesture, kind: ValueKind) -> Self {
        Self {
            binding,
            gesture,
            kind,
            child: None,
            edge: None,
            side: BindingSide::Write,
        }
    }

    const fn under(mut self, child: &'static str) -> Self {
        self.child = Some(child);
        self
    }

    const fn model(mut self) -> Self {
        self.side = BindingSide::ModelWrite;
        self
    }

    const fn edge(mut self, edge: Edge) -> Self {
        self.edge = Some(edge);
        self
    }
}

pub(crate) const fn primary(control: &ControlNode) -> Option<(Gesture, ValueKind)> {
    Some(match control {
        ControlNode::Pressable { .. }
        | ControlNode::Button { .. }
        | ControlNode::NavItem { .. }
        | ControlNode::TabLarge { .. }
        | ControlNode::Toggle { .. }
        | ControlNode::Checkbox { .. }
        | ControlNode::Chip { .. }
        | ControlNode::SettingsButton { .. } => (Gesture::Press, ValueKind::Trigger),
        ControlNode::Crossfader { .. }
        | ControlNode::Fader { .. }
        | ControlNode::Knob { .. }
        | ControlNode::Vis { .. }
        | ControlNode::VuStereo { .. }
        | ControlNode::VuVertical { .. }
        | ControlNode::Wave { .. } => (Gesture::Scalar, ValueKind::Scalar),
        ControlNode::Range { .. } => (Gesture::Scalar, ValueKind::Range),
        ControlNode::Segmented { .. }
        | ControlNode::ContextBar { .. }
        | ControlNode::Table { .. }
        | ControlNode::Tree { .. }
        | ControlNode::Select { .. } => (Gesture::Index, ValueKind::Index),
        ControlNode::PresetSelector { .. } => (Gesture::Text, ValueKind::Text),
        ControlNode::Row { .. } | ControlNode::Column { .. } => (Gesture::Step, ValueKind::Scalar),
        ControlNode::Placed { .. } => (Gesture::Place, ValueKind::Point),
        ControlNode::Bpm { .. }
        | ControlNode::DeckSummary { .. }
        | ControlNode::Text { .. }
        | ControlNode::Readout { .. }
        | ControlNode::Optional { .. }
        | ControlNode::Popover { .. }
        | ControlNode::Adaptive { .. }
        | ControlNode::Time { .. }
        | ControlNode::Scalar { .. }
        | ControlNode::Meter { .. }
        | ControlNode::Sprite { .. }
        | ControlNode::Lottie { .. }
        | ControlNode::Object { .. }
        | ControlNode::PortalMap { .. }
        | ControlNode::Include { .. }
        | ControlNode::Reveal { .. }
        | ControlNode::Scroll { .. }
        | ControlNode::Stage { .. }
        | ControlNode::Slot { .. }
        | ControlNode::Brand { .. }
        | ControlNode::Spacer { .. }
        | ControlNode::Divider { .. }
        | ControlNode::WindowDrag { .. }
        | ControlNode::TitleBar { .. }
        | ControlNode::WindowControls { .. }
        | ControlNode::Glyph { .. }
        | ControlNode::StatusDot { .. }
        | ControlNode::Swatch { .. }
        | ControlNode::Cell { .. }
        | ControlNode::Custom { .. }
        | ControlNode::Shader { .. } => return None,
    })
}

pub(crate) fn write_slots(site: ControlSite<'_>) -> Vec<WriteSlot<'_>> {
    let mut slots: Vec<WriteSlot<'_>> = Vec::new();
    if let (Some(write), Some((gesture, kind))) = (site.write, primary(site.control)) {
        let slot = WriteSlot::new(write, gesture, kind);
        match site.control {
            ControlNode::Range { .. } => {
                slots.push(slot.under("min").edge(Edge::Min));
                slots.push(slot.under("max").edge(Edge::Max));
            }
            ControlNode::ContextBar { .. } => slots.push(slot.model()),
            _ => slots.push(slot),
        }
    }
    let writes = site.writes;
    for (binding, gesture, kind, child, side) in [
        (
            writes.secondary,
            Gesture::Secondary,
            ValueKind::Trigger,
            None,
            BindingSide::Write,
        ),
        (
            writes.reset,
            Gesture::Press,
            ValueKind::Trigger,
            None,
            BindingSide::Write,
        ),
        (
            writes.loop_start,
            Gesture::Scalar,
            ValueKind::Scalar,
            Some("loop_start"),
            BindingSide::Write,
        ),
        (
            writes.loop_end,
            Gesture::Scalar,
            ValueKind::Scalar,
            Some("loop_end"),
            BindingSide::Write,
        ),
        (
            writes.zoom,
            Gesture::Scalar,
            ValueKind::Scalar,
            Some("zoom"),
            BindingSide::ModelWrite,
        ),
        (
            writes.query,
            Gesture::Text,
            ValueKind::Text,
            Some("search"),
            BindingSide::ModelWrite,
        ),
    ] {
        if let Some(binding) = binding {
            slots.push(WriteSlot {
                binding,
                child,
                gesture,
                kind,
                side,
                edge: None,
            });
        }
    }
    slots
}

pub(crate) fn column_writes(site: ControlSite<'_>) -> Vec<(String, BindingRef)> {
    let Some(binding) = site.writes.width else {
        return Vec::new();
    };
    site.columns
        .iter()
        .map(|column| {
            let mut scoped = binding.clone();
            if let BindingRef::Command { with, .. }
            | BindingRef::Parameter { with, .. }
            | BindingRef::Model { with, .. }
            | BindingRef::Telemetry { with, .. } = &mut scoped
            {
                with.insert("column".to_owned(), column.id().to_owned());
            }
            (format!("width/{}", column.id()), scoped)
        })
        .collect()
}
