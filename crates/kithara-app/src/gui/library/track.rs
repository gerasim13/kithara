use kithara::ui::render::{TableCell, TableRow};

/// One playable source a page lists.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct Track {
    title: String,
    #[field(get, vis = "pub(super)")]
    analysis_key: String,
    /// The source as given; what a deck is handed when the row is dropped.
    #[field(get, vis = "pub(super)")]
    url: String,
}

impl Track {
    pub(super) fn new(title: String, url: String) -> Self {
        Self {
            title,
            analysis_key: crate::catalog::canonical_source(&url),
            url,
        }
    }

    pub(super) fn row(&self, selected: bool) -> TableRow<'_> {
        TableRow::new(vec![TableCell::text("title", &self.title)], selected)
            .with_drag(self.url.as_str())
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
