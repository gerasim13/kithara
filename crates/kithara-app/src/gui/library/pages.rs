use std::collections::BTreeMap;

use kithara::ui::{
    ids::{DocId, EndpointId, NodeId},
    module::{BindingRef, ControlNode, ModuleDoc},
    registry::{EndpointCategory, EndpointDesc},
    source::MemResolver,
};
use kithara_app_library::{Document, PAGES, Registration};

use crate::gui::ui::endpoints::Registry;

/// What the registered sources add to the package: the module that mounts
/// their pages beside the modules they bring, their captions and the
/// endpoints their pages declare.
pub(in crate::gui) struct PagesModule {
    pub(in crate::gui) modules: MemResolver,
    pub(in crate::gui) registry: Registry,
    pub(in crate::gui) texts: Vec<Document>,
}

impl PagesModule {
    pub(in crate::gui) fn new(sources: &[Registration]) -> Self {
        let mut modules = MemResolver::default();
        let mut texts: Vec<Document> = Vec::new();
        let mut endpoints: Vec<(EndpointCategory, EndpointId, EndpointDesc)> = Vec::new();
        let mut children: Vec<ControlNode> = Vec::with_capacity(sources.len());
        for source in sources {
            let page = source.page();
            for module in &page.modules {
                modules.insert(module.path, module.text);
            }
            texts.extend(page.texts.iter().copied());
            endpoints.extend(page.endpoints.iter().map(|endpoint| {
                (
                    endpoint.category,
                    EndpointId(format!("source.{}", endpoint.name)),
                    EndpointDesc::new(endpoint.value).with_scope("source"),
                )
            }));
            let scope = BTreeMap::from([("source".to_owned(), page.id.to_owned())]);
            children.push(ControlNode::Optional {
                id: NodeId(page.id.to_owned()),
                hidden: BindingRef::Model {
                    id: EndpointId("library.page.hidden".to_owned()),
                    with: scope.clone(),
                },
                child: Box::new(ControlNode::Include {
                    id: NodeId(format!("{}-page", page.id)),
                    source: page.page.to_owned(),
                    with: scope,
                }),
            });
        }
        modules.insert_module(
            PAGES,
            ModuleDoc::new(
                DocId("library-pages".to_owned()),
                ControlNode::Stage {
                    children,
                    id: NodeId("pages".to_owned()),
                    size: None,
                },
            ),
        );
        Self {
            modules,
            texts,
            registry: Registry::default().with_endpoints(endpoints),
        }
    }
}
