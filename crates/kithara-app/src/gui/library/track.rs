use std::{borrow::Cow, collections::BTreeMap};

use kithara::ui::render::{TableCell, TableRow};
use kithara_app_library::Playable;

use crate::catalog::canonical_source;

/// One playable source a page lists.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct Track {
    title: String,
    /// The source as given; what a deck is handed when the row is dropped.
    #[field(get, vis = "pub(super)")]
    url: String,
    /// The source as the queue names it, which analysis is keyed by.
    #[field(get, vis = "pub(super)")]
    key: String,
    drag: BTreeMap<String, String>,
}

impl Track {
    pub(super) fn new(title: String, url: String) -> Self {
        Self {
            title,
            key: canonical_source(&url),
            drag: Playable::new(url.clone()).into(),
            url,
        }
    }

    pub(super) fn row(&self, selected: bool) -> TableRow<'_> {
        TableRow::new(vec![TableCell::text("title", &self.title)], selected)
            .with_drag(Cow::Borrowed(&self.drag))
    }
}

pub(super) fn display_name(url: &str) -> String {
    url.rsplit('/')
        .find(|segment| !segment.is_empty())
        .map_or_else(
            || url.to_string(),
            |segment| {
                segment
                    .rsplit_once('.')
                    .map_or_else(|| segment.to_string(), |(stem, _)| stem.to_string())
            },
        )
}
