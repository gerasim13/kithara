use iced::{Element, widget::Space};

use super::{mount::Cx, read_scope, resolve};
use crate::{
    atoms::{
        bar::context::{Context, Viewed},
        track_list::{TrackListRowData, column_layouts},
    },
    compile::CompiledUi,
    draw::Rect,
    expand::Binding,
    ids::InternId,
    module::TrackColumn,
    render::{
        InputOwner, ReadValue, Reads, Skin, Tree, UiEvent, Widget, controls::Paint, scope_picker,
        vis,
    },
    text::TextContext,
};

/// The context strip, with the menu its scope face opens.
///
/// The strip is one painted control on both hosts; only the menu is raised by
/// the toolkit, and it hangs off the face the painter reports rather than off
/// the strip.
pub(super) fn context_bar<'a>(
    cx: &Cx<'a, '_, '_>,
    scope: (&[InternId], Option<&Binding>),
    painter: Context,
    data: Viewed,
) -> Element<'a, UiEvent> {
    let (scope_items, scope) = scope;
    let skin = cx.skin;
    if scope_items.is_empty() {
        return Paint::new(painter, data, skin).view();
    }
    let scope_value = scope.and_then(|binding| resolve(cx.reads, binding, cx.ui));
    let mut text = TextContext::from(skin.text_resources());
    let Some(face) = painter.face(&mut text, &data) else {
        return Paint::new(painter, data, skin).view();
    };
    scope_picker(
        cx.path,
        scope_items.iter().map(|id| cx.ui.resolve(*id)).collect(),
        scope_value.as_ref(),
        skin,
        cx.owner,
        Paint::new(painter, data, skin).view(),
        move |bounds: Rect| Context::placed(face, bounds),
    )
}

pub(super) fn vis<'a>(value: Option<&ReadValue<'_>>, reads: &dyn Reads) -> Element<'a, UiEvent> {
    vis::view(value, reads)
}

pub(super) fn track_list<'a>(
    cx: &Cx<'a, '_, '_>,
    columns: (&[TrackColumn], Option<&Binding>),
) -> Element<'a, UiEvent> {
    let Some(ReadValue::TrackList(tracks)) = cx.value else {
        return Space::new().into();
    };
    let (columns, columns_state) = columns;
    let columns_scope = read_scope(columns_state, cx.ui);
    let columns_state = columns_state.map(|binding| cx.ui.resolve(binding.id));
    let state = columns_state.map(|prefix| (prefix, columns_scope));
    let columns = column_layouts(columns, cx.reads, state, cx.skin);
    let rows = tracks.iter().map(TrackListRowData::from).collect();
    crate::render::track_list(cx.path, rows, columns, cx.skin, cx.owner)
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
