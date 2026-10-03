use kithara::ui::{
    ids::{DocId, EndpointId, NodeId},
    module::{BindingRef, ControlNode, ModuleDoc},
    source::MemResolver,
};

use super::Registration;

pub(in crate::gui) struct PagesModule {
    document: ModuleDoc,
}

impl PagesModule {
    pub(in crate::gui) const PATH: &str = "library-pages.kmodule.ron";

    pub(in crate::gui) fn new(sources: &[Registration]) -> Self {
        let children = sources
            .iter()
            .map(|source| {
                let page = source.page();
                ControlNode::Optional {
                    id: NodeId(page.id.to_owned()),
                    hidden: BindingRef::Model {
                        id: EndpointId("library.page.hidden".to_owned()),
                        with: [("source".to_owned(), page.id.to_owned())].into(),
                    },
                    child: Box::new(page.page.clone()),
                }
            })
            .collect();
        Self {
            document: ModuleDoc::new(
                DocId("library-pages".to_owned()),
                ControlNode::Stage {
                    id: NodeId("pages".to_owned()),
                    size: None,
                    children,
                },
            ),
        }
    }
}

impl From<PagesModule> for MemResolver {
    fn from(pages: PagesModule) -> Self {
        let mut modules = Self::default();
        modules.insert_module(PagesModule::PATH, pages.document);
        modules
    }
}
