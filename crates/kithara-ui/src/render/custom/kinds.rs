use super::{mounted::MappedCustom, widget::CustomWidget};
use crate::render::{UiEvent, custom::MountedCustom};

/// What the application registers under one extension kind: how to build a
/// fresh widget, already speaking the document's own event vocabulary.
type Factory = Box<dyn Fn() -> Box<dyn MountedCustom<UiEvent>>>;

/// The extensions an application offers its hosts, named by kind.
///
/// One value, handed to whichever hosts draw the document, so both draw the
/// same extension for the same name. A document naming a kind absent here is
/// refused while it compiles, by [`crate::UiConfig::custom_kinds`].
#[derive(Default)]
pub struct CustomKinds {
    kinds: std::collections::BTreeMap<String, Factory>,
}

impl CustomKinds {
    /// Registers `make` under `kind`, mapping what its widget recognises into
    /// the document event vocabulary.
    #[must_use]
    pub fn with<Kind, Widget, Make, Map>(mut self, kind: Kind, make: Make, map: Map) -> Self
    where
        Kind: Into<String>,
        Widget: CustomWidget,
        Make: Fn() -> Widget + 'static,
        Map: Fn(Widget::Action) -> UiEvent + Clone + 'static,
    {
        self.kinds.insert(
            kind.into(),
            Box::new(move || Box::new(MappedCustom::new(make(), map.clone()))),
        );
        self
    }

    /// The names this registry answers for, which is what a document may name.
    #[must_use]
    pub fn names(&self) -> std::collections::BTreeSet<String> {
        self.kinds.keys().cloned().collect()
    }

    pub(crate) fn make(&self, kind: &str) -> Option<Box<dyn MountedCustom<UiEvent>>> {
        self.kinds.get(kind).map(|make| make())
    }
}
