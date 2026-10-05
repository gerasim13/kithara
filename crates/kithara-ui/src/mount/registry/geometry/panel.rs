use crate::{expand::Binding, ids::InternId, module::{TableColumn, TableFrame}, mount::{ContextBar, Lottie, Sprite, Table, Tree}};

pub(crate) fn lottie<'a>(
    _artwork: InternId, _active_artwork: Option<InternId>, _active: Option<&'a Binding>, _seconds: f32
) -> Lottie {
    Lottie::builder().build()
}

pub(crate) fn sprite(
    _sheet: InternId, _seconds: f32
) -> Sprite {
    Sprite::builder().build()
}

pub(crate) fn table<'a>(
    _presentation: (&'a [TableColumn], Option<&'a Binding>, Option<&'a Binding>, TableFrame, Option<&'a Binding>)
) -> Table {
    Table::builder().build()
}

pub(crate) fn tree<'a>(
    _query: Option<&'a Binding>, _search: bool, _toggle: bool
) -> Tree {
    Tree::builder().build()
}

pub(crate) fn context_bar<'a>(
    _scope_items: &'a [InternId], _scope: Option<&'a Binding>
) -> ContextBar {
    ContextBar::builder().build()
}
