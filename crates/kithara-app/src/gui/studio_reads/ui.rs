use kithara_ui::render::{Node, ReadValue, Scope};
use num_traits::cast::AsPrimitive;

use super::value::Value;
use crate::gui::studio_ui::{
    cache::{CollapsedModules, DeckLayout, SettingsSection, SettingsView},
    scope::deck_index,
};

#[derive(Clone, Copy)]
pub(super) struct UiNode<'a> {
    collapsed: &'a CollapsedModules,
    layout: DeckLayout,
    settings: &'a SettingsView,
    drag: DragNode<'a>,
}

impl<'a> UiNode<'a> {
    pub(super) const fn new(
        drag: DragNode<'a>,
        layout: DeckLayout,
        collapsed: &'a CollapsedModules,
        settings: &'a SettingsView,
    ) -> Self {
        Self {
            collapsed,
            layout,
            settings,
            drag,
        }
    }
}

impl<'a> Node<'a> for UiNode<'a> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let node: Box<dyn Node<'a> + 'a> = match segment {
            "drag" => Box::new(self.drag),
            "layout" => Box::new(LayoutNode {
                layout: self.layout,
            }),
            "module" => Box::new(ModulesNode {
                collapsed: self.collapsed,
            }),
            "settings" => Box::new(SettingsNode {
                settings: self.settings,
            }),
            _ => return None,
        };
        Some(node)
    }
}

#[derive(Clone, Copy)]
struct SettingsNode<'a> {
    settings: &'a SettingsView,
}

impl<'a> Node<'a> for SettingsNode<'a> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let section = self.settings.section;
        let value = match segment {
            "open" => ReadValue::Bool(self.settings.open),
            "on_view" => ReadValue::Bool(section == SettingsSection::View),
            "on_audio" => ReadValue::Bool(section == SettingsSection::Audio),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

#[derive(Clone, Copy)]
pub(super) struct DragNode<'a> {
    over: Option<usize>,
    track: Option<&'a str>,
    decks: usize,
}

impl<'a> DragNode<'a> {
    pub(super) const fn new(track: Option<&'a str>, over: Option<usize>, decks: usize) -> Self {
        Self { over, track, decks }
    }
}

impl<'a> Node<'a> for DragNode<'a> {
    fn child(&self, segment: &str, scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "track" => ReadValue::Text(self.track?),
            "over" => {
                let deck = deck_index(scope.get("deck")?).filter(|deck| *deck < self.decks)?;
                ReadValue::Bool(self.over == Some(deck))
            }
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

#[derive(Clone, Copy)]
struct LayoutNode {
    layout: DeckLayout,
}

impl<'a> Node<'a> for LayoutNode {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "decks" => ReadValue::Scalar(self.layout.index().as_()),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

#[derive(Clone, Copy)]
struct ModulesNode<'a> {
    collapsed: &'a CollapsedModules,
}

impl<'a> Node<'a> for ModulesNode<'a> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        Some(Box::new(ModuleNode {
            collapsed: self.collapsed.contains(segment),
        }))
    }
}

#[derive(Clone, Copy)]
struct ModuleNode {
    collapsed: bool,
}

impl<'a> Node<'a> for ModuleNode {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "collapsed" => ReadValue::Bool(self.collapsed),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}
