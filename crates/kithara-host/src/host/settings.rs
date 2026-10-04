use kithara_config::Config;

use crate::api::Tempo;

/// What a Host runs with that changes while it runs.
///
/// A change of one field goes to the Host through
/// [`kithara_config::Configure`], at the next block or at a session frame;
/// the getters read the settings as the render last applied them.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(default, builder(state_mod(vis = "pub")), fields(value, get(copy)))]
#[non_exhaustive]
pub struct HostSettings {
    /// Tempo the session transport counts beats in, 120 BPM unless changed.
    #[config(live(owner), builder(default = Tempo::DEFAULT))]
    tempo: Tempo,
}
