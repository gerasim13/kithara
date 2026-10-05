use kithara_app_library::{Document, Endpoint, SourcePage};
use kithara_ui::registry::{
    EndpointCategory::{Command, Model},
    ValueKind::{Bool, Text},
};

use super::consts;

/// The source's page, the captions its branch and page are worded with, and
/// the reads and commands the page declares.
pub(super) fn page() -> SourcePage {
    const PAGE: &str = "modules/library/zvuk-page.kmodule.ron";
    SourcePage {
        id: consts::ID,
        page: PAGE,
        modules: vec![Document {
            path: PAGE,
            text: include_str!("../../assets/zvuk-page.kmodule.ron"),
        }],
        texts: vec![Document {
            path: "texts/zvuk-en.ktext.ron",
            text: include_str!("../../assets/zvuk-en.ktext.ron"),
        }],
        endpoints: [
            (Model, "query", Text),
            (Command, "query", Text),
            (Model, "count", Text),
            (Command, "like_track", Text),
            (Model, "fault", Text),
            (Model, "fault_hidden", Bool),
        ]
        .into_iter()
        .map(|(category, name, value)| Endpoint {
            category,
            name,
            value,
        })
        .collect(),
    }
}
