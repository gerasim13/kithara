use std::rc::Rc;

use kithara_platform::time::Instant;

use super::custom::{HostAction, Repaint};
use crate::{
    atoms::{
        button::{Button, ButtonLabel, VisualState},
        chip::Chip,
        knob::Knob,
        nav_item::NavItem,
    },
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome, PointerOwnership, PointerPhase,
        recognizers::{Scalar, ScalarState, Track, WheelStep, click},
    },
    render::{ControlAction, Skin, UiEvent, control_event},
    text::TextContext,
};

/// One built-in control mounted as a Masonry leaf.
///
/// Built-ins take this route rather than the public custom-component contract
/// because they need direct cursor and capture ownership, which that contract
/// does not expose.
pub(crate) trait MasonryControl {
    fn draw_list(&mut self, bounds: Rect) -> DrawList;

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction>;

    fn accepts_input(&self) -> bool;

    fn cursor(&self, _hit: &Hit) -> CursorShape {
        CursorShape::None
    }

    /// Takes the hover edge the host observed and reports whether the control
    /// now draws differently.
    fn hover(&mut self, _hovered: bool) -> bool {
        false
    }

    fn repaint(&self) -> Repaint {
        Repaint::None
    }
}

pub(crate) struct MasonryKnob {
    input: Option<KnobInput>,
    label: Option<String>,
    painter: Knob,
    text: TextContext,
}

struct KnobInput {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
    recognizer: Scalar,
    state: ScalarState,
}

impl MasonryKnob {
    pub(crate) fn new(label: Option<String>, value: f32, skin: &Skin) -> Self {
        Self {
            input: None,
            label,
            painter: Knob::new(value, skin),
            text: TextContext::from(skin.text_resources()),
        }
    }

    pub(crate) fn interactive(
        mut self,
        path: String,
        track: Track,
        wheel: WheelStep,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        self.input = Some(KnobInput {
            map_event,
            path,
            recognizer: Scalar::builder()
                .track(track)
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(0.5)
                .wheel(wheel)
                .build(),
            state: ScalarState::default(),
        });
        self
    }
}

impl MasonryControl for MasonryKnob {
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        let mut list = DrawListBuilder::default();
        self.painter
            .paint(&mut list, &mut self.text, self.label.as_deref(), bounds);
        list.finish()
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        let Some(state) = &mut self.input else {
            return Outcome::IGNORED;
        };
        let had_pointer = state.state.captures_pointer();
        let outcome = state
            .recognizer
            .on_input(&mut state.state, input, hit, Instant::now());
        if matches!(
            input,
            Input::Pointer(pointer)
                if matches!(pointer.phase, PointerPhase::Cancel | PointerPhase::DoubleClick)
        ) {
            state.state.cancel_pointer();
        }
        let ownership = match (had_pointer, state.state.captures_pointer()) {
            (false, true) => PointerOwnership::Claim,
            (true, false) => PointerOwnership::Release,
            _ => PointerOwnership::Unchanged,
        };
        outcome.with_ownership(ownership).map(|value| {
            (state.map_event)(control_event(
                &state.path,
                ControlAction::SetScalar(f64::from(value)),
            ))
        })
    }

    fn accepts_input(&self) -> bool {
        self.input.is_some()
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.input.as_ref().map_or(CursorShape::None, |input| {
            input.recognizer.cursor(&input.state, hit)
        })
    }
}

/// A neutral painter that a press activates: the whole gesture is where the
/// press landed, so the host needs nothing from it but its drawing.
pub(crate) trait ClickPainter {
    /// The words this control draws. Most carry one; a button carries the pair
    /// it swaps between while active.
    type Label;

    /// Whether the pointer resting on or pressing the control changes what it
    /// draws, which decides if the host tracks and repaints on those edges.
    const READS_POINTER: bool = false;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &Self::Label,
        bounds: Rect,
        state: VisualState,
    );
}

impl ClickPainter for Chip {
    type Label = String;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &Self::Label,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, label, bounds);
    }
}

impl ClickPainter for NavItem {
    type Label = String;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &Self::Label,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, label, bounds);
    }
}

impl ClickPainter for Button {
    type Label = ButtonLabel<String>;

    const READS_POINTER: bool = true;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        label: &Self::Label,
        bounds: Rect,
        state: VisualState,
    ) {
        self.paint(list, text, label, bounds, state);
    }
}

/// Any control whose only gesture is a press, mounted as a Masonry leaf.
pub(crate) struct Click<Painter>
where
    Painter: ClickPainter,
{
    activation: Option<Activation>,
    label: Painter::Label,
    painter: Painter,
    press: Press,
    repaint: bool,
    text: TextContext,
}

/// Path and event mapping for a control that owns its own press.
struct Activation {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
}

/// What the pointer is doing to a control right now, for painters that draw
/// differently under it.
#[derive(Default)]
struct Press {
    hovered: bool,
    pressed: bool,
}

impl Press {
    const fn visual(&self) -> VisualState {
        if self.pressed && self.hovered {
            VisualState::Pressed
        } else if self.hovered {
            VisualState::Hovered
        } else {
            VisualState::Idle
        }
    }

    fn track(&mut self, input: Input<'_>, hit: &Hit) -> bool {
        let Input::Pointer(pointer) = input else {
            return false;
        };
        let pressed = match pointer.phase {
            PointerPhase::Down => hit.over(),
            PointerPhase::Cancel
            | PointerPhase::DoubleClick
            | PointerPhase::Leave
            | PointerPhase::Up => false,
            PointerPhase::LongPress | PointerPhase::Move | PointerPhase::MoveLongPress => {
                self.pressed
            }
        };
        std::mem::replace(&mut self.pressed, pressed) != pressed
    }
}

impl<Painter> Click<Painter>
where
    Painter: ClickPainter,
{
    pub(crate) fn new(painter: Painter, label: Painter::Label, skin: &Skin) -> Self {
        Self {
            activation: None,
            label,
            painter,
            press: Press::default(),
            repaint: false,
            text: TextContext::from(skin.text_resources()),
        }
    }

    pub(crate) fn interactive(
        mut self,
        path: String,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        self.activation = Some(Activation { map_event, path });
        self
    }
}

impl<Painter> MasonryControl for Click<Painter>
where
    Painter: ClickPainter,
{
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        self.repaint = false;
        let mut list = DrawListBuilder::default();
        self.painter.draw(
            &mut list,
            &mut self.text,
            &self.label,
            bounds,
            self.press.visual(),
        );
        list.finish()
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        if Painter::READS_POINTER {
            self.repaint |= self.press.track(input, hit);
        }
        self.activation
            .as_ref()
            .map_or(Outcome::IGNORED, |activation| {
                click::on_input(input, hit).map(|()| {
                    (activation.map_event)(control_event(&activation.path, ControlAction::Activate))
                })
            })
    }

    fn accepts_input(&self) -> bool {
        self.activation.is_some()
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.activation.as_ref().map_or(CursorShape::None, |_| {
            Hover::new(CursorShape::Pointer).cursor(self.press.pressed, hit)
        })
    }

    fn hover(&mut self, hovered: bool) -> bool {
        if !Painter::READS_POINTER {
            return false;
        }
        self.repaint |= std::mem::replace(&mut self.press.hovered, hovered) != hovered;
        self.repaint
    }

    fn repaint(&self) -> Repaint {
        if self.repaint {
            Repaint::NextFrame
        } else {
            Repaint::None
        }
    }
}
