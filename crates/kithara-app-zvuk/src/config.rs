use serde::Deserialize;

/// The `sources.zvuk` entry.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub(crate) user_agent: String,
}
