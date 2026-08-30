use std::rc::Rc;

use masonry::core::EventCtx;

use super::custom::HostAction;
use crate::{
    draw::Pt,
    interact::{CursorShape, Hit, Input, PointerOwnership, recognizers::Carry},
    render::{ControlAction, Snap, UiEvent, control_event},
};

/// One placement of a stage: where in the scene its child stands, and — where
/// the document gave the placement somewhere to write — the grip that carries
/// it.
///
/// The point moves the box the child is laid out in rather than offsetting
/// what it draws, so the region that answers the pointer travels with the
/// picture.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Spot {
    grip: Option<Grip>,
    #[field(get(copy), vis = "pub(crate)")]
    at: Pt,
}

/// What carries a placement: the gesture both hosts share, the path the point
/// is published on, and the magnet that takes the point before it is.
pub(crate) struct Grip {
    carry: Carry,
    snap: Option<Snap>,
    map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    path: String,
}

impl Grip {
    pub(crate) fn new(
        path: String,
        snap: Option<Snap>,
        map_event: Rc<dyn Fn(UiEvent) -> HostAction>,
    ) -> Self {
        Self {
            path,
            snap,
            map_event,
            carry: Carry::default(),
        }
    }
}

impl Spot {
    pub(crate) const fn new(at: Pt, grip: Option<Grip>) -> Self {
        Self { at, grip }
    }

    /// Offers one input to the grip, answering whether the grip took it.
    ///
    /// The hit is the node's own box, so the corner the gesture answers with
    /// is how far the placement has moved from where it stands; the point
    /// published is that far along in the scene.
    pub(crate) fn carry(&mut self, ctx: &mut EventCtx<'_>, input: Input<'_>, hit: &Hit) -> bool {
        let at = self.at;
        let Some(grip) = &mut self.grip else {
            return false;
        };
        let held = grip.carry.is_carried();
        let outcome = grip.carry.on_input(input, hit);
        match outcome.ownership() {
            PointerOwnership::Claim => ctx.capture_pointer(),
            PointerOwnership::Release if held => ctx.release_pointer(),
            PointerOwnership::Unchanged | PointerOwnership::Release => {}
        }
        if let Some(corner) = outcome.value() {
            let moved = Pt {
                x: at.x + corner.x,
                y: at.y + corner.y,
            };
            let moved = grip.snap.as_ref().map_or(moved, |snap| snap.take(moved));
            ctx.submit_action::<HostAction>((grip.map_event)(control_event(
                &grip.path,
                ControlAction::Place(moved),
            )));
        }
        let taken = outcome.is_captured() || held;
        if taken {
            ctx.set_handled();
        }
        taken
    }

    pub(crate) fn cursor(&self) -> CursorShape {
        self.grip
            .as_ref()
            .map_or(CursorShape::None, |grip| grip.carry.cursor())
    }

    /// Whether a pointer over this placement may carry it.
    pub(crate) const fn grips(&self) -> bool {
        self.grip.is_some()
    }

    /// Moves the placement to the point the document now answers, saying
    /// whether that is somewhere else than it stood.
    pub(crate) fn move_to(&mut self, at: Pt) -> bool {
        let moved = self.at != at;
        self.at = at;
        moved
    }
}
