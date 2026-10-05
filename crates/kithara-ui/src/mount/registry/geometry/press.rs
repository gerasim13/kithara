use crate::{
    ids::InternId,
    layout::FrameSides,
    module::{ButtonStyle, ChipStyle, IconName},
    mount::{Button, Chip, NavItem, Segmented, Tab},
};

pub(crate) fn nav_item(_label: InternId, _icon: IconName) -> NavItem {
    NavItem::builder().build()
}

pub(crate) fn tab(_label: InternId) -> Tab {
    Tab::builder().build()
}

pub(crate) fn button(
    style: ButtonStyle,
    _presentation: (
        InternId,
        Option<IconName>,
        Option<InternId>,
        Option<FrameSides>,
    ),
) -> Button {
    Button::builder().style(style).build()
}

pub(crate) fn segmented<'a>(_items: &'a [InternId]) -> Segmented {
    Segmented::builder().build()
}

pub(crate) fn chip(_label: InternId, _style: ChipStyle) -> Chip {
    Chip::builder().build()
}
