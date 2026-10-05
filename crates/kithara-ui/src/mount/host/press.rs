use crate::{
    ids::InternId,
    layout::FrameSides,
    module::{ButtonStyle, ChipStyle, IconName},
    mount::{Button, Chip, NavItem, Segmented, Tab},
};

pub(crate) fn nav_item(label: InternId, icon: IconName) -> NavItem {
    NavItem::builder().icon(icon).label(label).build()
}

pub(crate) fn tab(label: InternId) -> Tab {
    Tab::builder().label(label).build()
}

pub(crate) fn button(
    style: ButtonStyle,
    (label, icon, active_label, frame): (
        InternId,
        Option<IconName>,
        Option<InternId>,
        Option<FrameSides>,
    ),
) -> Button {
    Button::builder()
        .style(style)
        .maybe_active_label(active_label)
        .maybe_frame(frame)
        .maybe_icon(icon)
        .label(label)
        .build()
}

pub(crate) fn segmented<'a>(items: &'a [InternId]) -> Segmented<'a> {
    Segmented::builder().items(items).build()
}

pub(crate) fn chip(label: InternId, style: ChipStyle) -> Chip {
    Chip::builder().label(label).style(style).build()
}
