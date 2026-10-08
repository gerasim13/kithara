use crate::{
    render::{
        Published,
        custom::{CustomKinds, CustomWidget, TextMeasurer},
    },
    shaping::TextContext,
};

impl CustomKinds {
    pub(crate) fn make(&self, kind: &str) -> Option<Box<dyn CustomWidget<Action = Published>>> {
        self.kinds.get(kind).map(|make| make())
    }
}

impl<'a> TextMeasurer<'a> {
    pub(crate) const fn new(context: &'a mut TextContext) -> Self {
        Self { context }
    }
}
