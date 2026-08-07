use iced::Element;

use super::{read_scope, resolve};
use crate::{
    compile::CompiledUi,
    expand::Binding,
    ids::InternId,
    module::{DeckSummaryStyle, TrackColumn},
    render::{InputOwner, ReadValue, Reads, Skin, UiEvent},
    widgets::{
        Widget,
        deck::DeckSummary,
        nav::{ContextBar, Tree},
        track_list::TrackList,
        vis::Vis,
    },
};

pub(super) fn deck_summary<'a>(
    style: DeckSummaryStyle,
    value: Option<&ReadValue<'_>>,
    scope: &str,
    reads: &dyn Reads,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    DeckSummary::builder()
        .style(style)
        .maybe_value(value)
        .scope(scope)
        .reads(reads)
        .skin(skin)
        .build()
        .view()
}

pub(super) fn vis<'a>(value: Option<&ReadValue<'_>>, reads: &dyn Reads) -> Element<'a, UiEvent> {
    Vis::builder()
        .maybe_preset(value)
        .reads(reads)
        .build()
        .view()
}

pub(super) fn track_list<'a>(
    path: &'a str,
    columns: (&[TrackColumn], Option<&Binding>),
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let (columns, columns_state) = columns;
    TrackList::builder()
        .path(path)
        .columns(columns)
        .maybe_columns_state(columns_state.map(|binding| ui.resolve(binding.id)))
        .columns_scope(read_scope(columns_state, ui))
        .maybe_value(value)
        .reads(reads)
        .skin(skin)
        .owner(owner)
        .build()
        .view()
}

pub(super) fn tree<'a>(
    path: &'a str,
    query: Option<&Binding>,
    value: Option<&ReadValue<'_>>,
    ui: &CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let query = query
        .and_then(|binding| resolve(reads, binding, ui))
        .and_then(|value| match value {
            ReadValue::Text(query) => Some(query),
            _ => None,
        })
        .unwrap_or_default();
    Tree::builder()
        .path(path)
        .query(query)
        .maybe_value(value)
        .owner(owner)
        .skin(skin)
        .build()
        .view()
}

pub(super) fn context_bar<'a>(
    path: &'a str,
    scope: (&[InternId], Option<&Binding>),
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    reads: &dyn Reads,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let (scope_items, scope) = scope;
    let scope_value = scope.and_then(|binding| resolve(reads, binding, ui));
    ContextBar::builder()
        .path(path)
        .scope_items(scope_items.iter().map(|id| ui.resolve(*id)).collect())
        .maybe_scope_value(scope_value.as_ref())
        .maybe_value(value)
        .skin(skin)
        .owner(owner)
        .build()
        .view()
}
