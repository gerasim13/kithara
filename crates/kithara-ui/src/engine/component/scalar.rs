use kithara_platform::time::Instant;

use super::retained::Component;
use crate::{
    engine::model::{EngineEvent, Kind},
    interact::{
        CursorShape, Hit, Input, Outcome,
        recognizers::{Scalar, ScalarState},
    },
};

pub(in crate::engine) struct ScalarComponent {
    path: String,
    kind: Kind,
    scalar: Scalar,
    state: ScalarState,
}

impl ScalarComponent {
    pub(super) fn new(path: String, kind: Kind, scalar: Scalar) -> Self {
        Self {
            path,
            kind,
            scalar,
            state: ScalarState::default(),
        }
    }

    pub(super) fn reconcile(mut self, next: Self) -> Self {
        if self.kind != next.kind {
            return next;
        }
        self.path = next.path;
        self.scalar = next.scalar;
        self
    }
}

impl Component for ScalarComponent {
    fn path(&self) -> &str {
        &self.path
    }

    fn kind(&self) -> Kind {
        self.kind
    }

    fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<EngineEvent> {
        self.scalar
            .on_input(&mut self.state, input, hit, now)
            .map(EngineEvent::Scalar)
    }

    fn cursor(&self, hit: &Hit) -> CursorShape {
        self.scalar.cursor(&self.state, hit)
    }

    fn captures_pointer(&self) -> bool {
        self.state.captures_pointer()
    }
}
