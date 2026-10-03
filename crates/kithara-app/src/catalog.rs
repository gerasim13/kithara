use kithara::{
    prelude::ResourceSrc,
    queue::{QueueError, Transition},
};

use crate::{config::AppConfig, pools::AppQueueControl, sources::build_source};

#[must_use]
pub(crate) fn canonical_source(url: &str) -> String {
    ResourceSrc::parse(url).map_or_else(|_| url.to_string(), |src| src.to_string())
}

/// Put the track at `url` on `queue` and make it current. A track already on
/// this deck is selected rather than appended, so loading twice is a no-op plus
/// a select.
///
/// # Errors
/// Returns [`QueueError`] when the queue rejects the selection.
pub fn load_onto(queue: &AppQueueControl, url: &str, config: &AppConfig) -> Result<(), QueueError> {
    let source = canonical_source(url);
    let existing = queue
        .tracks()
        .into_iter()
        .find(|track| track.url.as_deref() == Some(source.as_str()))
        .map(|track| track.id);
    let id = match existing {
        Some(id) => id,
        None => queue.append(build_source(url, config))?,
    };
    queue.select(id, Transition::None)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test(native, flash(false))]
    fn a_source_is_normalized_like_queue_urls() {
        assert_eq!(
            canonical_source("HTTPS://Example.COM:443/a.mp3"),
            "https://example.com/a.mp3"
        );
        assert_eq!(canonical_source("/music/b.mp3"), "/music/b.mp3");
        assert_eq!(canonical_source("not a source"), "not a source");
    }
}
