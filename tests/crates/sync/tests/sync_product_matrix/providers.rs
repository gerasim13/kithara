use super::{cases::*, imports::*};
pub(crate) use crate::providers::{HlsProtection, Provider};
use crate::providers::{LIBRARY, sources};

pub(crate) const AMBIENT_TRIP_HOP_PROVIDER: Provider = Provider::Rhythm(AMBIENT_TRIP_HOP);

pub(crate) const DOWNTEMPO_HOUSE_PROVIDER: Provider = Provider::Rhythm(DOWNTEMPO_HOUSE);

pub(crate) const TECHNO_BREAKBEAT_PROVIDER: Provider = Provider::Rhythm(TECHNO_BREAKBEAT);

pub(crate) const CROSS_STYLE_PROVIDER: Provider = Provider::Rhythm(CROSS_STYLE);

impl Provider {
    pub(crate) const ALL: &[Self] = &[
        Self::Synthetic,
        Self::Rhythm(CROSS_STYLE),
        Self::HlsSame(HlsProtection::Plain),
        Self::HlsSame(HlsProtection::Drm),
        Self::Library(LIBRARY),
        Self::Library(TUNNEL),
        Self::Library(NEWTECHNO),
        Self::Mp3Same,
        Self::Mp3Distinct,
        Self::HlsMp3(HlsProtection::Plain),
        Self::HlsMp3(HlsProtection::Drm),
        Self::Sweep,
    ];

    pub(crate) const fn has_score_markers(self) -> bool {
        matches!(self, Self::Rhythm(_))
    }

    /// The beat grid deck `deck`'s track was rendered or analysed on.
    pub(crate) fn beat_grid(self, deck: usize) -> ArtifactSource<BeatGridModel> {
        match self {
            Self::Synthetic => synthetic_grid(),
            Self::Library(names) => {
                ArtifactSource::Value(Arc::new(analysed_grid(names[deck % names.len()])))
            }
            other => panic!("{other:?} declares no beat grid for a gridded case"),
        }
    }

    /// The second deck `deck`'s track opens at for `start`.
    pub(crate) fn start_seconds(self, deck: usize, start: Start) -> f64 {
        let grid = match self {
            Self::Library(names) => Some(analysed_grid(names[deck % names.len()])),
            _ => None,
        };
        start.seconds(grid.as_ref())
    }
}

pub(crate) type PreparedSources = (Provider, TestServerHelper, Vec<String>);

pub(crate) async fn prepared_sources(provider: Provider) -> PreparedSources {
    let server = TestServerHelper::new().await;
    let paths = sources(provider, 4, &server).await;
    (provider, server, paths)
}
