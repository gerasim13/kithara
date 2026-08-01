use iced::{Element, widget::canvas::Action};

use crate::{
    engine::EngineEvent,
    interact::{
        Outcome,
        recognizers::{DragEvent, StepEvent},
    },
};

/// Shared view contract: a built control renders itself into the event tree.
pub(crate) trait Widget<'a> {
    fn view(self) -> Element<'a, UiEvent>;
}

/// Carry a recognizer's verdict out to the toolkit. The recognizer decides
/// whether the gesture took the pointer; this only names the event it produced.
fn action<T>(outcome: Outcome<T>, event: impl FnOnce(T) -> UiEvent) -> Option<Action<UiEvent>> {
    let captured = outcome.is_captured();
    let Some(value) = outcome.value() else {
        return captured.then(Action::capture);
    };
    let published = Action::publish(event(value));
    Some(if captured {
        published.and_capture()
    } else {
        published
    })
}

/// The one place a control event is built. Every publisher and every widget
/// goes through here, so a binding rule has a single site to attach to instead
/// of fifteen literals to keep in step.
pub(crate) fn control_event(path: &str, action: ControlAction) -> UiEvent {
    UiEvent::Control {
        path: path.to_owned(),
        action,
    }
}

fn set_scalar(path: &str, value: f32) -> UiEvent {
    control_event(path, ControlAction::SetScalar(f64::from(value)))
}

pub(crate) fn scalar(path: &str, outcome: Outcome) -> Option<Action<UiEvent>> {
    action(outcome, |value| set_scalar(path, value))
}

pub(crate) fn step(path: &str, outcome: Outcome<StepEvent>) -> Option<Action<UiEvent>> {
    action(outcome, |event| {
        control_event(
            path,
            match event {
                StepEvent::By(steps) => ControlAction::StepScalar(steps),
                StepEvent::Activate => ControlAction::Activate,
            },
        )
    })
}

pub(crate) fn activate(path: &str, outcome: Outcome<()>) -> Option<Action<UiEvent>> {
    action(outcome, |()| control_event(path, ControlAction::Activate))
}

pub(crate) fn engine(path: &str, outcome: Outcome<EngineEvent>) -> Option<Action<UiEvent>> {
    let captured = outcome.is_captured();
    match outcome.value() {
        Some(EngineEvent::Scalar(value)) => scalar(path, typed_outcome(value, captured)),
        Some(EngineEvent::Activate) => activate(path, typed_outcome((), captured)),
        None => captured.then(Action::capture),
    }
}

fn typed_outcome<T>(value: T, captured: bool) -> Outcome<T> {
    if captured {
        Outcome::set(value)
    } else {
        Outcome::observed(value)
    }
}

/// A value a control decides for itself, addressed under one of its own
/// endpoints rather than the one its gesture writes.
pub(crate) fn scalar_child(path: &str, child: &str, value: f32) -> Action<UiEvent> {
    Action::publish(set_scalar(&format!("{path}/{child}"), value)).and_capture()
}

pub(crate) fn drag(
    path: &str,
    index: usize,
    outcome: Outcome<DragEvent>,
) -> Option<Action<UiEvent>> {
    action(outcome, |event| {
        control_event(
            path,
            ControlAction::Drag(match event {
                DragEvent::Started => DragPhase::Start(index),
                DragEvent::Dropped => DragPhase::Drop,
            }),
        )
    })
}

/// Action emitted by an interactive control.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ControlAction {
    Activate,
    SecondaryActivate,
    SetScalar(f64),
    StepScalar(f32),
    SelectIndex(usize),
    Drag(DragPhase),
}

/// Phase of a pointer drag that carries an item from the control it started on
/// to the one it is released over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DragPhase {
    /// The item at this index is now being dragged out of the control.
    Start(usize),
    /// The pointer crossed into (`true`) or out of (`false`) the control.
    Over(bool),
    /// The pointer was released and the drag ended.
    Drop,
}

/// Command emitted by portable window-chrome controls and executed by the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WindowCommand {
    Drag,
    Resize(WindowEdge),
    Minimize,
    ToggleMaximize,
    Close,
}

/// Which side or corner of the window a resize drag pulls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WindowEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// Event emitted by the shared UI contract.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum UiEvent {
    Control { path: String, action: ControlAction },
    SelectPreset(String),
    ToggleModule(String),
    OpenSettings,
    LibraryQuery(String),
    Window(WindowCommand),
}
