use kithara::ui::render::{Node, ReadValue, Scope};

use super::value::{Value, impl_child_node};
use crate::gui::ui::{cache::DeckLayout, modules::Modules, window::WindowState};

#[derive(Clone, Copy)]
pub(super) struct UiNode<'a> {
    modules: &'a Modules,
    window: &'a WindowState,
    layout: DeckLayout,
    config_path: &'a str,
}

impl<'a> UiNode<'a> {
    pub(super) const fn new(
        layout: DeckLayout,
        modules: &'a Modules,
        window: &'a WindowState,
        config_path: &'a str,
    ) -> Self {
        Self {
            modules,
            window,
            layout,
            config_path,
        }
    }
}

impl_child_node!(UiNode<'a>, |this, segment, _scope| {
    let node: Box<dyn Node<'a> + 'a> = match segment {
        "app" => Box::new(AppNode {
            config_path: this.config_path,
        }),
        "layout" => Box::new(LayoutNode {
            layout: this.layout,
        }),
        "layouts" => Box::new(LayoutsNode {
            layout: this.layout,
        }),
        "window" => Box::new(WindowNode {
            window: this.window,
        }),
        "module" => Box::new(ModulesNode {
            modules: this.modules,
        }),
        "modules" => Box::new(ModuleCountNode {
            modules: this.modules,
        }),
        _ => return None,
    };
    Some(node)
});

#[derive(Clone, Copy)]
struct AppNode<'a> {
    config_path: &'a str,
}

impl_child_node!(AppNode<'a>, |this, segment, _scope| {
    let value = match segment {
        "version" => ReadValue::Text(env!("CARGO_PKG_VERSION")),
        "config_path" => ReadValue::Text(this.config_path),
        _ => return None,
    };
    Some(Box::new(Value(value)))
});

#[derive(Clone, Copy)]
struct WindowNode<'a> {
    window: &'a WindowState,
}

impl_child_node!(WindowNode<'a>, |this, segment, scope| {
    let only = scope.get("window") == Some("1");
    let value = match segment {
        "count" => ReadValue::Text("1 WINDOW"),
        "active" => ReadValue::Bool(only),
        "close_hidden" => ReadValue::Bool(true),
        "chrome_hidden" => ReadValue::Bool(this.window.chrome_hidden()),
        "title" => ReadValue::Text(this.window.title()),
        "caption" => ReadValue::Text(this.window.caption()),
        _ => return None,
    };
    Some(Box::new(Value(value)))
});

#[derive(Clone, Copy)]
struct LayoutNode {
    layout: DeckLayout,
}

impl<'a> Node<'a> for LayoutNode {
    fn child(&self, segment: &str, scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "selected" => ReadValue::Bool(self.is_selected(scope)),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

impl LayoutNode {
    /// A menu row names its layout by deck count; a count the app has no
    /// layout for is never the one in force.
    fn is_selected(self, scope: Scope<'_>) -> bool {
        scope
            .get("layout")
            .and_then(|decks| decks.parse().ok())
            .and_then(DeckLayout::from_decks)
            == Some(self.layout)
    }
}

#[derive(Clone, Copy)]
struct LayoutsNode {
    layout: DeckLayout,
}

impl<'a> Node<'a> for LayoutsNode {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "active" => ReadValue::Text(self.layout.label()),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

#[derive(Clone, Copy)]
struct ModulesNode<'a> {
    modules: &'a Modules,
}

impl_child_node!(ModulesNode<'a>, |this, segment, scope| {
    let value = match segment {
        "on" => ReadValue::Bool(this.is_on(scope)),
        "hidden" => ReadValue::Bool(!this.is_on(scope)),
        _ => return None,
    };
    Some(Box::new(Value(value)))
});

impl ModulesNode<'_> {
    fn is_on(self, scope: Scope<'_>) -> bool {
        scope
            .get("module")
            .is_some_and(|module| self.modules.is_on(module))
    }
}

#[derive(Clone, Copy)]
struct ModuleCountNode<'a> {
    modules: &'a Modules,
}

impl<'a> Node<'a> for ModuleCountNode<'a> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "count" => ReadValue::Text(self.modules.count()),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}
