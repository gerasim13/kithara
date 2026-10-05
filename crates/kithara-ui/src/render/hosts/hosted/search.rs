use std::{cell::RefCell, rc::Rc};

#[cfg(feature = "masonry")]
use super::masonry::SearchState;
use super::plan::Resolving;
use crate::{
    atoms::search::Search, engine::Descriptor, expand::Binding, render::text_input_layout,
};

/// The shared search face and the text-input descriptor that drives it.
#[derive(Clone)]
pub(crate) struct SearchPlan {
    pub(crate) path: String,
    pub(in crate::render) picture: Rc<RefCell<Search>>,
    #[cfg(feature = "masonry")]
    pub(super) state: SearchState,
}

impl SearchPlan {
    pub(super) fn new(path: &str, query: &str, _read: Option<&Binding>, cx: Resolving<'_>) -> Self {
        let plan = Self {
            path: path.to_owned(),
            picture: Rc::new(RefCell::new(Search::new(query, cx.skin))),
            #[cfg(feature = "masonry")]
            state: SearchState::default(),
        };
        #[cfg(feature = "masonry")]
        plan.bind_source(_read.map(|binding| cx.ctx.ui.resolve(binding.key).to_owned()));
        plan
    }

    pub(super) fn descriptor(&self) -> Descriptor {
        let picture = self.picture.borrow();
        Descriptor::text_input(
            self.path.clone(),
            picture.query().to_owned(),
            text_input_layout(picture.query(), picture.skin()),
        )
    }
}
