use std::collections::BTreeMap;

use crate::{
    hosts::event::control_event,
    render::{Carry, ControlAction, Published},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Carried {
    pub(crate) data: BTreeMap<String, String>,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct DragSession {
    carried: Option<Carried>,
    over: Option<String>,
}

impl DragSession {
    pub(crate) fn follow(
        &mut self,
        event: &Published,
        carried: impl FnOnce(&str, usize) -> Option<Carried>,
    ) -> Option<Published> {
        let Published::Carry { path, step } = event else {
            return None;
        };
        match step {
            Carry::Start(index) => {
                self.carried = carried(path, *index);
                None
            }
            Carry::Over(true) => {
                self.over = Some(path.clone());
                None
            }
            Carry::Over(false) => {
                if self.over.as_ref() == Some(path) {
                    self.over = None;
                }
                None
            }
            Carry::Drop => {
                let carried = self.carried.take()?;
                let zone = self.over.as_deref()?;
                Some(control_event(zone, ControlAction::Record(carried.data)))
            }
        }
    }

    pub(crate) fn hovered(&self) -> Option<&str> {
        self.carried.as_ref().and(self.over.as_deref())
    }

    pub(crate) fn label(&self) -> Option<&str> {
        self.carried.as_ref()?.label.as_deref()
    }
}
