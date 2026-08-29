use std::collections::BTreeMap;

use super::control::check_scopes;
use crate::{
    error::UiDocError,
    ids::{EndpointId, SourceUri},
    module::{BindingRef, ControlNode},
    registry::{EndpointCategory, EndpointRegistry, ValueKind},
};

pub(super) const BLOCK_HIDDEN: ValueKind = ValueKind::Bool;

#[derive(Clone, Copy)]
pub(super) enum BindingSide {
    Read,
    Write,
    ModelWrite,
}

pub(crate) const fn value_kinds(control: &ControlNode) -> (Option<ValueKind>, Option<ValueKind>) {
    match control {
        ControlNode::Bpm { .. } => (Some(ValueKind::Waveform), None),
        ControlNode::DeckSummary { .. }
        | ControlNode::Text { .. }
        | ControlNode::Readout { .. } => (Some(ValueKind::Text), None),
        ControlNode::ContextBar { .. } => (Some(ValueKind::Text), Some(ValueKind::Scalar)),
        ControlNode::Optional { .. } => (Some(BLOCK_HIDDEN), None),
        ControlNode::Popover { .. } => (Some(ValueKind::Bool), None),
        ControlNode::Pressable { .. } => (None, Some(ValueKind::Trigger)),
        ControlNode::Button { .. }
        | ControlNode::NavItem { .. }
        | ControlNode::TabLarge { .. }
        | ControlNode::Toggle { .. }
        | ControlNode::Checkbox { .. }
        | ControlNode::Chip { .. } => (Some(ValueKind::Bool), Some(ValueKind::Trigger)),
        // An adaptive block reads the number that picks its branch, a sprite
        // how far its sheet has run, an artwork how far its pass has, and an
        // object how far its motion has, and nothing writes back through any
        // of them.
        ControlNode::Adaptive { .. }
        | ControlNode::Time { .. }
        | ControlNode::Scalar { .. }
        | ControlNode::Meter { .. }
        | ControlNode::Sprite { .. }
        | ControlNode::Lottie { .. }
        | ControlNode::Object { .. } => (Some(ValueKind::Scalar), None),
        ControlNode::Crossfader { .. }
        | ControlNode::Fader { .. }
        | ControlNode::Knob { .. }
        | ControlNode::Segmented { .. }
        | ControlNode::Vis { .. } => (Some(ValueKind::Scalar), Some(ValueKind::Scalar)),
        ControlNode::Wave { .. } => (Some(ValueKind::Waveform), Some(ValueKind::Scalar)),
        ControlNode::PortalMap { .. } => (Some(ValueKind::PortalMap), None),
        ControlNode::Range { .. } => (Some(ValueKind::Range), Some(ValueKind::Scalar)),
        ControlNode::Table { .. } => (Some(ValueKind::Table), None),
        ControlNode::Tree { .. } => (Some(ValueKind::Tree), None),
        ControlNode::VuStereo { .. } | ControlNode::VuVertical { .. } => {
            (Some(ValueKind::Stereo), Some(ValueKind::Scalar))
        }
        ControlNode::Row { .. } | ControlNode::Column { .. } => (None, Some(ValueKind::Scalar)),
        // A placement reads the point it stands on and publishes the point a
        // drag leaves it on, which is the same point either way round.
        ControlNode::Placed { .. } => (Some(ValueKind::Point), Some(ValueKind::Point)),
        ControlNode::Include { .. }
        | ControlNode::Reveal { .. }
        | ControlNode::Scroll { .. }
        | ControlNode::Stage { .. }
        | ControlNode::Slot { .. }
        | ControlNode::Brand { .. }
        | ControlNode::Spacer { .. }
        | ControlNode::Divider { .. }
        | ControlNode::PresetSelector { .. }
        | ControlNode::SettingsButton { .. }
        | ControlNode::WindowDrag { .. }
        | ControlNode::TitleBar { .. }
        | ControlNode::WindowControls { .. }
        | ControlNode::Glyph { .. }
        | ControlNode::Select { .. }
        | ControlNode::StatusDot { .. }
        | ControlNode::Swatch { .. }
        | ControlNode::Cell { .. }
        | ControlNode::Custom { .. }
        | ControlNode::Shader { .. } => (None, None),
    }
}

pub(super) const fn binding_parts(
    binding: &BindingRef,
) -> (EndpointCategory, &EndpointId, &BTreeMap<String, String>) {
    match binding {
        BindingRef::Command { id, with } => (EndpointCategory::Command, id, with),
        BindingRef::Parameter { id, with } => (EndpointCategory::Parameter, id, with),
        BindingRef::Telemetry { id, with } => (EndpointCategory::Telemetry, id, with),
        BindingRef::Model { id, with } => (EndpointCategory::Model, id, with),
    }
}

pub(super) fn check_binding(
    binding: &BindingRef,
    side: BindingSide,
    expected_kind: Option<ValueKind>,
    path: &str,
    origin: &SourceUri,
    endpoints: &dyn EndpointRegistry,
) -> Result<(), UiDocError> {
    let (category, id, with) = binding_parts(binding);
    let allowed = match side {
        BindingSide::Read => matches!(
            category,
            EndpointCategory::Parameter | EndpointCategory::Telemetry | EndpointCategory::Model
        ),
        BindingSide::Write => matches!(
            category,
            EndpointCategory::Command | EndpointCategory::Parameter
        ),
        BindingSide::ModelWrite => {
            matches!(
                category,
                EndpointCategory::Command | EndpointCategory::Parameter | EndpointCategory::Model
            )
        }
    };
    if !allowed {
        return Err(UiDocError::BindingDirection {
            origin: origin.clone(),
            id: id.0.clone(),
            path: path.to_owned(),
            detail: format!("{category} endpoint is not allowed on this side"),
        });
    }
    let Some(endpoint) = endpoints.endpoint(category, id) else {
        return Err(UiDocError::UnknownEndpoint {
            origin: origin.clone(),
            category: category.to_string(),
            id: id.0.clone(),
            path: path.to_owned(),
        });
    };
    let Some(expected_kind) = expected_kind else {
        return Err(UiDocError::BindingDirection {
            origin: origin.clone(),
            id: id.0.clone(),
            path: path.to_owned(),
            detail: "control does not support this side".to_owned(),
        });
    };
    if expected_kind != endpoint.value {
        return Err(UiDocError::BindingType {
            origin: origin.clone(),
            id: id.0.clone(),
            path: path.to_owned(),
            expected: expected_kind.to_string(),
            got: endpoint.value.to_string(),
        });
    }
    check_scopes(id, with, endpoint, path, origin)
}
