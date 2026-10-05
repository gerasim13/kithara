use serde::Deserialize;

/// The `sources.zvuk` entry: the client identity and the account token
/// catalogue requests carry.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub(crate) auth_token: String,
    pub(crate) user_agent: String,
}
