use crate::size::{Dim, SizeSpec};

/// The strip under the tree that names the scope in view.
#[derive(kithara_derive::Control)]
#[control(size = SizeSpec::new(Dim::Fill, Dim::Fixed(skin.tree.context_height)))]
pub(crate) struct ContextBar;

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{expand::Binding, ids::InternId};

    #[derive(Builder)]
    pub(crate) struct ContextBar<'a> {
        pub(crate) scope_items: &'a [InternId],
        pub(crate) scope: Option<&'a Binding>,
    }

    use crate::{
        atoms::bar::context::{Context, Scope, Viewed},
        hosts::{
            controls::{Draws, Reading},
            picker::picker_selected_index,
        },
        render::{ReadValue, Skin},
    };

    impl Draws for ContextBar<'_> {
        type Painter = Context;

        /// The path in view is what this strip is for: without one there is
        /// nothing to name, and the scope beside it is what the document
        /// offered rather than what an endpoint reports.
        fn data(&self, read: Reading<'_>) -> Option<Viewed> {
            let ReadValue::Text(breadcrumb) = read.value? else {
                return None;
            };
            let items = self
                .scope_items
                .iter()
                .map(|item| read.ctx.ui.resolve(*item).to_owned())
                .collect::<Vec<String>>();
            Some(Viewed {
                breadcrumb: (*breadcrumb).to_owned(),
                scope: (!items.is_empty()).then(|| Scope {
                    selected: picker_selected_index(
                        self.scope
                            .and_then(|binding| read.ctx.read(binding))
                            .as_ref(),
                        items.len(),
                    ),
                    items,
                }),
            })
        }

        fn painter(&self, skin: &Skin) -> Context {
            Context::new(skin)
        }
    }
}
