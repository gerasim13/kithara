use crate::engine::{
    Engine,
    component::retained::{Component, RetainedComponent},
    model::Kind,
};

impl Engine {
    pub(crate) fn clear_focus(&mut self) {
        self.router.clear_focus(&mut self.components);
    }

    pub(crate) fn column_divider_value(&self, path: &str) -> Option<f32> {
        self.components
            .iter()
            .find(|component| component.path() == path && component.kind() == Kind::ColumnDivider)
            .and_then(RetainedComponent::column_divider_value)
    }
}

impl RetainedComponent {
    pub(crate) fn column_divider_value(&self) -> Option<f32> {
        if let Self::Scalar(component) = self
            && component.kind() == Kind::ColumnDivider
        {
            component.current
        } else {
            None
        }
    }
}
