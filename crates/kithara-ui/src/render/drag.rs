use crate::render::{Carry, CarryStep, ControlAction, Published, control_event};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Carried {
    pub(crate) data: String,
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
        let Published::Carry {
            path,
            step: Carry(step),
        } = event
        else {
            return None;
        };
        match step {
            CarryStep::Start(index) => {
                self.carried = carried(path, *index);
                None
            }
            CarryStep::Over(true) => {
                self.over = Some(path.clone());
                None
            }
            CarryStep::Over(false) => {
                if self.over.as_ref() == Some(path) {
                    self.over = None;
                }
                None
            }
            CarryStep::Drop => {
                let carried = self.carried.take()?;
                let zone = self.over.as_deref()?;
                Some(control_event(zone, ControlAction::Text(carried.data)))
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
