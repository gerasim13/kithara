use kithara_platform::time::Instant;

use super::{
    activation::ActivationComponent, crossing::CrossingComponent, scalar::ScalarComponent,
    segmented::SegmentedComponent, wave::HeroWaveComponent,
};
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
    fn handle(
        &mut self,
        input: Input,
        hit: &Hit,
        now: Instant,
    ) -> (Outcome<EngineEvent>, Option<&'static str>);
    fn cursor(&self, hit: &Hit) -> CursorShape;
    fn captures_pointer(&self) -> bool;
}

pub(in crate::engine) enum RetainedComponent {
    Scalar(ScalarComponent),
    Activation(ActivationComponent),
    Crossing(CrossingComponent),
    Segmented(SegmentedComponent),
    HeroWave(HeroWaveComponent),
}

impl RetainedComponent {
    pub(in crate::engine) fn reconcile(self, descriptor: Descriptor) -> Self {
        let next = descriptor.into();
        match (self, next) {
            (Self::Scalar(component), Self::Scalar(next)) => {
                Self::Scalar(component.reconcile(next))
            }
            (Self::HeroWave(component), Self::HeroWave(next)) => {
                Self::HeroWave(component.reconcile(next))
            }
            (Self::Crossing(component), Self::Crossing(_)) => Self::Crossing(component),
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
    ) -> (Outcome<EngineEvent>, Option<&'static str>) {
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
            Self::Crossing(component) => component,
            Self::Segmented(component) => component,
            Self::HeroWave(component) => component,
        }
    }

    fn component_mut(&mut self) -> &mut dyn Component {
        match self {
            Self::Scalar(component) => component,
            Self::Activation(component) => component,
            Self::Crossing(component) => component,
            Self::Segmented(component) => component,
            Self::HeroWave(component) => component,
        }
    }
}

impl From<Descriptor> for RetainedComponent {
    fn from(descriptor: Descriptor) -> Self {
        match descriptor {
            Descriptor::Activation { path } => Self::Activation(ActivationComponent::new(path)),
            Descriptor::Crossing { path } => Self::Crossing(CrossingComponent::new(path)),
            Descriptor::Segmented { path, item_count } => {
                Self::Segmented(SegmentedComponent::new(path, item_count))
            }
            Descriptor::Fader {
                path,
                scalar,
                drag_step,
            } => Self::Scalar(ScalarComponent::new(path, Kind::Fader, scalar, drag_step)),
            Descriptor::Crossfader { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::Crossfader,
                Scalar::builder()
                    .track(Track::AbsoluteHorizontal)
                    .hover(Hover::new(CursorShape::ResizeH))
                    .build(),
                None,
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
                None,
            )),
            Descriptor::StereoMeter { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::StereoMeter,
                Scalar::builder()
                    .track(Track::AbsoluteHorizontal)
                    .hover(Hover::new(CursorShape::ResizeH))
                    .build(),
                None,
            )),
            Descriptor::VerticalVu { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::VerticalVu,
                Scalar::builder()
                    .track(Track::AbsoluteVertical)
                    .hover(Hover::new(CursorShape::ResizeV))
                    .build(),
                None,
            )),
            Descriptor::Wave { path } => Self::Scalar(ScalarComponent::new(
                path,
                Kind::Wave,
                Scalar::builder()
                    .track(Track::HorizontalClick)
                    .hover(Hover::new(CursorShape::Pointer))
                    .build(),
                None,
            )),
            Descriptor::HeroWave {
                path,
                scale,
                progress,
                visible,
                wheel_positive,
                wheel_non_positive,
            } => Self::HeroWave(HeroWaveComponent::new(
                path,
                scale,
                progress,
                visible,
                wheel_positive,
                wheel_non_positive,
            )),
        }
    }
}
