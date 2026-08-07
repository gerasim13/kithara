use iced::Element;

use crate::{
    atoms::design::segmented::Segmented,
    compile::CompiledUi,
    ids::InternId,
    render::{InputOwner, ReadValue, Skin, UiEvent},
    widgets::Widget,
};

pub(super) fn segmented<'a>(
    path: &'a str,
    items: &[InternId],
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    skin: &Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let segmented = Segmented::builder()
        .path(path)
        .items(items.iter().map(|id| ui.resolve(*id)).collect())
        .maybe_value(value)
        .skin(skin)
        .build();
    match owner {
        InputOwner::Leaf => segmented.view(),
        InputOwner::Engine => segmented.painted(),
    }
}
