use kithara_platform::time::Instant;

use super::{
    component::RetainedComponent,
    model::{Emission, EngineEvent, Identity, Target},
};
use crate::interact::{CursorShape, Input, Outcome};

#[derive(Default)]
pub(super) struct Router {
    capture: Option<Identity>,
}

impl Router {
    pub(super) fn reconcile(&mut self, components: &[RetainedComponent]) {
        let missing = self.capture.as_ref().is_some_and(|identity| {
            !components
                .iter()
                .any(|component| component.has_identity(identity))
        });
        if missing {
            self.capture = None;
        }
    }

    pub(super) fn handle(
        &mut self,
        components: &mut [RetainedComponent],
        input: Input,
        targets: &[Target<'_>],
        now: Instant,
    ) -> Option<Emission> {
        if let Some(identity) = &self.capture {
            let component_index = components
                .iter()
                .position(|component| component.has_identity(identity))?;
            let target = targets.iter().find(|target| target.path == identity.path)?;
            let component = &mut components[component_index];
            let outcome = component.handle(input, &target.hit, now);
            let path = component.path().to_owned();
            if !component.captures_pointer() {
                self.capture = None;
            }
            return emission(path, outcome);
        }

        for target in targets.iter().rev() {
            let Some(component) = components
                .iter_mut()
                .find(|component| component.path() == target.path)
            else {
                continue;
            };
            let outcome = component.handle(input, &target.hit, now);
            if outcome == Outcome::IGNORED {
                continue;
            }
            if component.captures_pointer() {
                self.capture = Some(component.identity());
            }
            return emission(component.path().to_owned(), outcome);
        }
        None
    }

    pub(super) fn cursor(
        &self,
        components: &[RetainedComponent],
        targets: &[Target<'_>],
    ) -> CursorShape {
        if let Some(identity) = &self.capture {
            let component = components
                .iter()
                .find(|component| component.has_identity(identity));
            let target = targets.iter().find(|target| target.path == identity.path);
            return component
                .zip(target)
                .map_or(CursorShape::None, |(component, target)| {
                    component.cursor(&target.hit)
                });
        }

        targets
            .iter()
            .rev()
            .filter_map(|target| {
                components
                    .iter()
                    .find(|component| component.path() == target.path)
                    .map(|component| component.cursor(&target.hit))
            })
            .find(|cursor| *cursor != CursorShape::None)
            .unwrap_or(CursorShape::None)
    }
}

fn emission(path: String, outcome: Outcome<EngineEvent>) -> Option<Emission> {
    (outcome != Outcome::IGNORED).then_some(Emission { path, outcome })
}
