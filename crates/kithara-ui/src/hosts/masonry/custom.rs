use masonry::core::ErasedAction;

pub(crate) use crate::render::custom::{CustomWidget, MappedCustom, Repaint};

/// One action on its way out of this host, still typed but no longer named.
#[derive(Debug)]
pub(crate) struct HostAction(ErasedAction);

impl HostAction {
    pub(crate) fn new<Action>(action: Action) -> Self
    where
        Action: std::fmt::Debug + Send + 'static,
    {
        Self(Box::new(action))
    }

    pub(crate) fn downcast<Action>(self) -> Result<Action, Self>
    where
        Action: std::fmt::Debug + Send + 'static,
    {
        self.0.downcast().map(|action| *action).map_err(Self)
    }

    delegate::delegate! {
        to self.0 {
            pub(crate) fn type_name(&self) -> &'static str;
        }
    }
}
