use crate::{ids::InternId, mount::TitleBar};

pub(crate) fn title_bar(
    label: InternId
) -> TitleBar {
    TitleBar::builder().label(label).build()
}
