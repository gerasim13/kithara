use crate::{
    interact::recognizers::Edge,
    render::{Carry, ControlAction, Published},
};

/// The one place a control event is built. Every publisher and every widget
/// goes through here, so a binding rule has a single site to attach to instead
/// of fifteen literals to keep in step.
pub(crate) fn control_event(path: &str, action: ControlAction) -> Published {
    Published::Gesture {
        action,
        path: path.to_owned(),
    }
}

pub(crate) fn carry_event(path: &str, step: Carry) -> Published {
    Published::Carry {
        step,
        path: path.to_owned(),
    }
}

/// Where one end of a two-handled interval publishes.
///
/// Both ends are host-owned scalars under the control's own path, so the
/// control needs no second document node and the host needs no rule for
/// turning an index back into a name.
pub(crate) fn span_event(path: &str, edge: Edge, value: f32) -> Published {
    let child = match edge {
        Edge::Min => "min",
        Edge::Max => "max",
    };
    control_event(
        &format!("{path}/{child}"),
        ControlAction::SetScalar(f64::from(value)),
    )
}
