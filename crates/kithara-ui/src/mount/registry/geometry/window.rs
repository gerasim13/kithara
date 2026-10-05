use crate::{ids::InternId, mount::TitleBar};

pub(crate) fn title_bar(
    _label: InternId
) -> TitleBar {
    TitleBar::builder().build()
}
