use kithara_platform::time::Instant;

use super::{
    knob::KnobComponent,
    model::{Descriptor, Identity, Kind},
    vu::VerticalVuComponent,
};
use crate::interact::{CursorShape, Hit, Input, Outcome};

pub(super) trait Component {
    fn path(&self) -> &str;
    fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<f32>;
    fn cursor(&self, hit: &Hit) -> CursorShape;
    fn captures_pointer(&self) -> bool;
}

pub(super) enum RetainedComponent {
    Knob(KnobComponent),
    VerticalVu(VerticalVuComponent),
}

impl RetainedComponent {
    pub(super) fn reconcile(self, descriptor: Descriptor) -> Self {
        match (self, descriptor) {
            (
                Self::Knob(mut component),
                Descriptor::Knob {
                    path,
                    current,
                    drag_range,
                    wheel_step,
                },
            ) => {
                component.refresh(path, current, drag_range, wheel_step);
                Self::Knob(component)
            }
            (Self::VerticalVu(mut component), Descriptor::VerticalVu { path }) => {
                component.refresh(path);
                Self::VerticalVu(component)
            }
            (_, descriptor) => descriptor.into(),
        }
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
        self.component().path()
    }

    pub(super) const fn kind(&self) -> Kind {
        match self {
            Self::Knob(_) => Kind::Knob,
            Self::VerticalVu(_) => Kind::VerticalVu,
        }
    }

    pub(super) fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<f32> {
        self.component_mut().handle(input, hit, now)
    }

    pub(super) fn cursor(&self, hit: &Hit) -> CursorShape {
        self.component().cursor(hit)
    }

    pub(super) fn captures_pointer(&self) -> bool {
        self.component().captures_pointer()
    }

    fn component(&self) -> &dyn Component {
        match self {
            Self::Knob(component) => component,
            Self::VerticalVu(component) => component,
        }
    }

    fn component_mut(&mut self) -> &mut dyn Component {
        match self {
            Self::Knob(component) => component,
            Self::VerticalVu(component) => component,
        }
    }
}

impl From<Descriptor> for RetainedComponent {
    fn from(descriptor: Descriptor) -> Self {
        match descriptor {
            Descriptor::Knob {
                path,
                current,
                drag_range,
                wheel_step,
            } => Self::Knob(KnobComponent::new(path, current, drag_range, wheel_step)),
            Descriptor::VerticalVu { path } => Self::VerticalVu(VerticalVuComponent::new(path)),
        }
    }
}
