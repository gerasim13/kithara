use crate::{
    expand::Binding,
    ids::InternId,
    module::{TableColumn, TableFrame},
    mount::{ContextBar, Lottie, Sprite, Table, Tree},
};

pub(crate) fn lottie<'a>(
    artwork: InternId,
    active_artwork: Option<InternId>,
    active: Option<&'a Binding>,
    seconds: f32,
) -> Lottie<'a> {
    Lottie::builder()
        .artwork(artwork)
        .maybe_active_artwork(active_artwork)
        .maybe_active(active)
        .seconds(seconds)
        .build()
}

pub(crate) fn sprite(sheet: InternId, seconds: f32) -> Sprite {
    Sprite::builder().seconds(seconds).sheet(sheet).build()
}

pub(crate) fn table<'a>(
    (columns, columns_state, status, frame, width): (
        &'a [TableColumn],
        Option<&'a Binding>,
        Option<&'a Binding>,
        TableFrame,
        Option<&'a Binding>,
    ),
) -> Table<'a> {
    Table::builder()
        .columns(columns)
        .maybe_columns_state(columns_state)
        .maybe_width(width)
        .frame(frame)
        .maybe_status(status)
        .build()
}

pub(crate) fn tree<'a>(query: Option<&'a Binding>, search: bool, toggle: bool) -> Tree<'a> {
    Tree::builder()
        .maybe_query(query)
        .search(search)
        .toggle(toggle)
        .build()
}

pub(crate) fn context_bar<'a>(
    scope_items: &'a [InternId],
    scope: Option<&'a Binding>,
) -> ContextBar<'a> {
    ContextBar::builder()
        .maybe_scope(scope)
        .scope_items(scope_items)
        .build()
}
