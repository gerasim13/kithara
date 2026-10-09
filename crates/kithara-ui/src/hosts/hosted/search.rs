use std::{cell::RefCell, rc::Rc};

use crate::{
    atoms::{search::Search, text_input::text_input_layout},
    engine::Descriptor,
    expand::Binding,
    hosts::hosted::{HostedState, Resolving},
};

/// The shared search face and the text-input descriptor that drives it.
#[derive(Clone)]
pub(crate) struct SearchPlan<S: HostedState> {
    pub(crate) path: String,
    pub(crate) picture: Rc<RefCell<Search>>,
    pub(crate) state: S::Search,
}

impl<S: HostedState> SearchPlan<S> {
    pub(super) fn new(path: &str, query: &str, read: Option<&Binding>, cx: Resolving<'_>) -> Self {
        let plan = Self {
            path: path.to_owned(),
            picture: Rc::new(RefCell::new(Search::new(query, cx.skin))),
            state: S::Search::default(),
        };
        S::bind_search(&plan.state, read, cx);
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
