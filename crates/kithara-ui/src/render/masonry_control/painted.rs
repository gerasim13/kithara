use std::rc::Rc;

use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use super::{
    controls::{MasonryControl, Retained},
    custom::{HostAction, Repaint},
};
use crate::{
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome, PointerOwnership, PointerPhase,
        recognizers::{Scalar, ScalarState, click},
    },
    render::{
        ControlAction, ReadValue, Skin, UiEvent, control_event,
        controls::{Drag, Grip, Press},
    },
    text::TextContext,
};

/// One built-in control mounted as a Masonry leaf: a painter, the data it
/// draws, and the gesture it may answer.
pub(crate) struct Painted<Painter>
where
    Painter: Retained,
{
    data: Painter::Data,
    interaction: Option<Interaction>,
    painter: Painter,
    press: Press,
    repaint: bool,
    text: TextContext,
}

/// What a control does with the pointer, and where it publishes the answer.
struct Interaction {
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
    recognize: Recognize,
}

enum Recognize {
    Press,
    Command(fn() -> UiEvent),
    Drag(Box<Dragged>),
}

/// A scalar drag in flight: what it was described as, the recognizer made from
/// that description, and the gesture's own state.
///
/// The recognizer is re-made whenever the value moves, because a relative track
/// counts travel from the value it was built with. The state is kept across
/// that, so a hand already dragging is not interrupted by its own answer coming
/// back from the application.
struct Dragged {
    recognizer: Scalar,
    spec: Drag,
    state: ScalarState,
}

impl Dragged {
    fn new(spec: Drag) -> Self {
        Self {
            recognizer: spec.recognizer(),
            spec,
            state: ScalarState::default(),
        }
    }

    fn at(&mut self, value: f32) {
        self.spec = self.spec.at(value);
        self.recognizer = self.spec.recognizer();
    }

    /// One input through the recognizer, carrying the pointer ownership the
    /// host needs to route the rest of the gesture to this leaf.
    fn follow(&mut self, input: Input<'_>, hit: &Hit) -> Outcome {
        let had_pointer = self.state.captures_pointer();
        let outcome = self
            .recognizer
            .on_input(&mut self.state, input, hit, Instant::now());
        if matches!(
            input,
            Input::Pointer(pointer)
                if matches!(pointer.phase, PointerPhase::Cancel | PointerPhase::DoubleClick)
        ) {
            self.state.cancel_pointer();
        }
        let ownership = match (had_pointer, self.state.captures_pointer()) {
            (false, true) => PointerOwnership::Claim,
            (true, false) => PointerOwnership::Release,
            _ => PointerOwnership::Unchanged,
        };
        outcome.with_ownership(ownership)
    }
}

impl<Painter> Painted<Painter>
where
    Painter: Retained,
{
    pub(crate) fn new(painter: Painter, data: Painter::Data, skin: &Skin) -> Self {
        Self {
            data,
            interaction: None,
            painter,
            press: Press::default(),
            repaint: false,
            text: TextContext::from(skin.text_resources()),
        }
    }

    pub(crate) fn interactive(
        mut self,
        grip: Grip,
        path: String,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        let recognize = match grip {
            Grip::None => return self,
            Grip::Press => Recognize::Press,
            Grip::Command(event) => Recognize::Command(event),
            Grip::Drag(drag) => Recognize::Drag(Box::new(Dragged::new(drag))),
            // Picking a cell is a gesture only the immediate host recognises;
            // here the engine plan drives it, which is what this host has
            // always done. The two are reconciled by the gesture census.
            Grip::Index { .. } => return self,
        };
        self.interaction = Some(Interaction {
            map_event,
            path,
            recognize,
        });
        self
    }

    /// The gesture is measured against the part of the box the painter says the
    /// pointer works, which for most controls is all of it.
    fn gripped(&self, hit: &Hit) -> Hit {
        Hit::new(hit.at(), self.painter.grip_bounds(&self.data, hit.area()))
    }

    /// Takes the value the control now draws into whatever counts from it.
    fn moved_to(&mut self, value: &ReadValue<'_>) {
        let (Some(interaction), ReadValue::Scalar(value)) = (&mut self.interaction, value) else {
            return;
        };
        if let Recognize::Drag(drag) = &mut interaction.recognize {
            drag.at(AsPrimitive::<f32>::as_(value.clamp(0.0, 1.0)));
        }
    }
}

impl<Painter> MasonryControl for Painted<Painter>
where
    Painter: Retained,
{
    fn draw_list(&mut self, bounds: Rect) -> DrawList {
        self.repaint = false;
        let mut list = DrawListBuilder::default();
        self.painter.draw(
            &mut list,
            &mut self.text,
            &self.data,
            bounds,
            self.press.visual(),
        );
        list.finish()
    }

    fn measure(&mut self) -> crate::solve::Size {
        self.painter.measure(&mut self.text, &self.data)
    }

    fn input(&mut self, input: Input<'_>, hit: &Hit) -> Outcome<HostAction> {
        if Painter::READS_POINTER {
            self.repaint |= self.press.press(input, hit);
        }
        let gripped = self.gripped(hit);
        let Some(interaction) = &mut self.interaction else {
            return Outcome::IGNORED;
        };
        let (outcome, spec) = match &mut interaction.recognize {
            Recognize::Press => {
                return click::on_input(input, hit).map(|()| {
                    (interaction.map_event)(control_event(
                        &interaction.path,
                        ControlAction::Activate,
                    ))
                });
            }
            Recognize::Command(event) => {
                let event = *event;
                return click::on_input(input, hit).map(|()| (interaction.map_event)(event()));
            }
            Recognize::Drag(drag) => (drag.follow(input, &gripped), drag.spec),
        };
        // The control draws the value it just authored: the application is told
        // the same number, but its answer only comes back a frame later.
        if let Some(value) = outcome.value() {
            self.repaint |= Painter::set_read(&mut self.data, &ReadValue::Scalar(f64::from(value)));
            self.moved_to(&ReadValue::Scalar(f64::from(value)));
        }
        let Some(interaction) = &self.interaction else {
            return Outcome::IGNORED;
        };
        outcome.map(|value| {
            (interaction.map_event)(control_event(
                &interaction.path,
                ControlAction::SetScalar(spec.published(input, value)),
            ))
        })
    }

    fn accepts_input(&self) -> bool {
        self.interaction.is_some()
    }

    fn set_read(&mut self, value: &ReadValue<'_>) -> bool {
        self.repaint |= Painter::set_read(&mut self.data, value);
        self.moved_to(value);
        self.repaint
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.interaction
            .as_ref()
            .map_or(CursorShape::None, |interaction| {
                match &interaction.recognize {
                    Recognize::Press | Recognize::Command(_) => {
                        Hover::new(CursorShape::Pointer).cursor(self.press.is_pressed(), hit)
                    }
                    Recognize::Drag(drag) => {
                        drag.recognizer.cursor(&drag.state, &self.gripped(hit))
                    }
                }
            })
    }

    fn hover(&mut self, hovered: bool) -> bool {
        if !Painter::READS_POINTER {
            return false;
        }
        self.repaint |= self.press.hover(hovered);
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
