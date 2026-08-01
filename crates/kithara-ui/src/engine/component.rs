use kithara_platform::time::Instant;

use super::model::{Descriptor, Identity, Kind};
use crate::interact::{
    CursorShape, Hit, Hover, Input, Outcome,
    recognizers::{Scalar, ScalarState, Track, WheelStep},
};

pub(super) struct ScalarComponent {
    path: String,
    kind: Kind,
    scalar: Scalar,
    state: ScalarState,
}

impl ScalarComponent {
    pub(super) fn reconcile(mut self, descriptor: Descriptor) -> Self {
        if self.kind != descriptor.kind() {
            return descriptor.into();
        }
        let (path, scalar) = config(descriptor);
        self.path = path;
        self.scalar = scalar;
        self
    }

    pub(super) fn identity(&self) -> Identity {
        Identity {
            path: self.path().to_owned(),
            kind: self.kind(),
        }
    }

    pub(super) fn has_identity(&self, identity: &Identity) -> bool {
        self.kind() == identity.kind && self.path() == identity.path
    }

    pub(super) fn path(&self) -> &str {
        &self.path
    }

    pub(super) const fn kind(&self) -> Kind {
        self.kind
    }

    pub(super) fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<f32> {
        self.scalar.on_input(&mut self.state, input, hit, now)
    }

    pub(super) fn cursor(&self, hit: &Hit) -> CursorShape {
        self.scalar.cursor(&self.state, hit)
    }

    pub(super) fn captures_pointer(&self) -> bool {
        self.state.captures_pointer()
    }
}

impl From<Descriptor> for ScalarComponent {
    fn from(descriptor: Descriptor) -> Self {
        let kind = descriptor.kind();
        let (path, scalar) = config(descriptor);
        Self {
            path,
            kind,
            scalar,
            state: ScalarState::default(),
        }
    }
}

fn config(descriptor: Descriptor) -> (String, Scalar) {
    match descriptor {
        Descriptor::Crossfader { path } | Descriptor::StereoMeter { path } => (
            path,
            Scalar::builder()
                .track(Track::AbsoluteHorizontal)
                .hover(Hover::new(CursorShape::ResizeH))
                .build(),
        ),
        Descriptor::Knob {
            path,
            current,
            drag_range,
            wheel_step,
        } => (
            path,
            Scalar::builder()
                .track(Track::RelativeVertical {
                    range: drag_range,
                    value: current,
                })
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(0.5)
                .wheel(WheelStep {
                    value: current,
                    step: wheel_step,
                })
                .build(),
        ),
        Descriptor::VerticalVu { path } => (
            path,
            Scalar::builder()
                .track(Track::AbsoluteVertical)
                .hover(Hover::new(CursorShape::ResizeV))
                .build(),
        ),
    }
}
