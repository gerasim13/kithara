use kithara_platform::time::Instant;

use super::{activation::ActivationComponent, scalar::ScalarComponent};
use crate::{
    engine::model::{Descriptor, EngineEvent, Identity, Kind},
    interact::{
        CursorShape, Hit, Hover, Input, Outcome,
        recognizers::{Scalar, Track, WheelStep},
    },
};

pub(super) trait Component {
    fn path(&self) -> &str;
    fn kind(&self) -> Kind;
    fn handle(&mut self, input: Input, hit: &Hit, now: Instant) -> Outcome<EngineEvent>;
    fn cursor(&self, hit: &Hit) -> CursorShape;
    fn captures_pointer(&self) -> bool;
}

pub(in crate::engine) enum RetainedComponent {
    Scalar(ScalarComponent),
    Activation(ActivationComponent),
}

impl RetainedComponent {
    pub(in crate::engine) fn reconcile(self, descriptor: Descriptor) -> Self {
        let next = descriptor.into();
        match (self, next) {
            (Self::Scalar(component), Self::Scalar(next)) => {
                Self::Scalar(component.reconcile(next))
            }
            (_, next) => next,
        }
    }

    pub(in crate::engine) fn identity(&self) -> Identity {
        Identity {
            path: self.path().to_owned(),
            kind: self.kind(),
        }
    }

    pub(in crate::engine) fn has_identity(&self, identity: &Identity) -> bool {
        self.kind() == identity.kind && self.path() == identity.path
    }

    pub(in crate::engine) fn path(&self) -> &str {
        self.component().path()
    }

    pub(in crate::engine) fn kind(&self) -> Kind {
        self.component().kind()
    }

    pub(in crate::engine) fn handle(
        &mut self,
        input: Input,
        hit: &Hit,
        now: Instant,
    ) -> Outcome<EngineEvent> {
        self.component_mut().handle(input, hit, now)
    }

    pub(in crate::engine) fn cursor(&self, hit: &Hit) -> CursorShape {
        self.component().cursor(hit)
    }

    pub(in crate::engine) fn captures_pointer(&self) -> bool {
        self.component().captures_pointer()
    }

    fn component(&self) -> &dyn Component {
        match self {
            Self::Scalar(component) => component,
            Self::Activation(component) => component,
        }
    }

    fn component_mut(&mut self) -> &mut dyn Component {
        match self {
            Self::Scalar(component) => component,
            Self::Activation(component) => component,
        }
    }
}

impl From<Descriptor> for RetainedComponent {
    fn from(descriptor: Descriptor) -> Self {
        match descriptor {
            Descriptor::Activation { path } => Self::Activation(ActivationComponent::new(path)),
            Descriptor::Crossfader { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::Crossfader,
                Scalar::builder()
                    .track(Track::AbsoluteHorizontal)
                    .hover(Hover::new(CursorShape::ResizeH))
                    .build(),
            )),
            Descriptor::Knob {
                path,
                current,
                drag_range,
                wheel_step,
            } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::Knob,
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
            )),
            Descriptor::StereoMeter { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::StereoMeter,
                Scalar::builder()
                    .track(Track::AbsoluteHorizontal)
                    .hover(Hover::new(CursorShape::ResizeH))
                    .build(),
            )),
            Descriptor::VerticalVu { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::VerticalVu,
                Scalar::builder()
                    .track(Track::AbsoluteVertical)
                    .hover(Hover::new(CursorShape::ResizeV))
                    .build(),
            )),
            Descriptor::Wave {
                path,
                beats_shown,
                scale,
                progress,
            } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::Wave,
                Scalar::builder()
                    .track(if beats_shown {
                        Track::RelativeHorizontal {
                            scale,
                            value: progress,
                        }
                    } else {
                        Track::HorizontalClick
                    })
                    .hover(Hover::new(if beats_shown {
                        CursorShape::Grab
                    } else {
                        CursorShape::Pointer
                    }))
                    .build(),
            )),
        }
    }
}
