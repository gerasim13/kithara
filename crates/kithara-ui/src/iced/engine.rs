use crate::engine::{Engine, TextInputSnapshot, component::scroll::ScrollState, model::Kind};

impl Engine {
    pub(crate) fn has_pressed_item(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.pressed_item_index().is_some())
    }

    pub(crate) fn pressed_item_index(&self, path: &str) -> Option<usize> {
        self.item_pressed(path).flatten()
    }

    /// Whether the focused component edits text, so pasted text belongs to it.
    pub(crate) fn editing_text(&self) -> bool {
        self.router.focused_path().is_some_and(|path| {
            self.components
                .iter()
                .any(|component| component.path() == path && component.kind() == Kind::TextInput)
        })
    }

    pub(crate) fn text_input_snapshots(&self) -> Vec<(String, TextInputSnapshot)> {
        let focused = self.router.focused_path();
        self.components
            .iter()
            .filter_map(|component| {
                component
                    .text_input_snapshot(focused == Some(component.path()))
                    .map(|snapshot| (component.path().to_owned(), snapshot))
            })
            .collect()
    }
}

impl ScrollState {
    pub(crate) fn sync_offset(&mut self, offset: f32) {
        self.offset = offset.clamp(0.0, self.max_offset());
    }
}
