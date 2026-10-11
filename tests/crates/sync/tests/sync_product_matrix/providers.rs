use super::{cases::*, imports::*};
use crate::providers::sources;
pub(crate) use crate::providers::{HlsProtection, Provider};

pub(crate) const AMBIENT_TRIP_HOP_PROVIDER: Provider = Provider::Rhythm(AMBIENT_TRIP_HOP);
pub(crate) const DOWNTEMPO_HOUSE_PROVIDER: Provider = Provider::Rhythm(DOWNTEMPO_HOUSE);
pub(crate) const TECHNO_BREAKBEAT_PROVIDER: Provider = Provider::Rhythm(TECHNO_BREAKBEAT);
pub(crate) const CROSS_STYLE_PROVIDER: Provider = Provider::Rhythm(CROSS_STYLE);

impl Provider {
    pub(crate) const fn has_score_markers(self) -> bool {
        matches!(self, Self::Rhythm(_))
    }

    /// The beat grid deck `deck`'s track was rendered or analysed on.
    pub(crate) fn beat_grid(self, deck: usize) -> ArtifactSource<BeatGridModel> {
        match self {
            Self::Library(names) => {
                ArtifactSource::Value(Arc::new(library_grid(names[deck % names.len()])))
            }
            Self::Rhythm(names) => {
                let name =
                    names[deck % names.len()].replace("rhythm_wav_", "rhythm_expected_analysis_");
                let asset = kithara_test_fixtures::assets::by_name(&name)
                    .unwrap_or_else(|| panic!("missing score grid {name}"));
                let fingerprint =
                    kithara::analysis::AnalysisFingerprint::new(Some("rhythm-fixture:v1"), None);
                let file = kithara::analysis::AnalysisFile::parse(asset.bytes(), &fingerprint)
                    .expect("fixture score analysis");
                let grid =
                    BeatGridModel::try_from(file.latest().analysis()).expect("fixture score grid");
                ArtifactSource::Value(Arc::new(grid))
            }
            _ => synthetic_grid(),
        }
    }

    /// The second deck `deck`'s track opens at for `start`.
    pub(crate) fn start_seconds(self, deck: usize, start: Start) -> f64 {
        let grid = match self {
            Self::Library(names) => Some(library_grid(names[deck % names.len()])),
            _ => None,
        };
        start.seconds(grid.as_ref())
    }
}

// Ruling: spec 4.5 needs strong beats for preparation; Newtechno's named phrase at beat 64 supplies the fixture's four-beat bar phase, not the conflicting detector votes.
fn library_grid(name: &str) -> BeatGridModel {
    let grid = analysed_grid(name);
    if name != NEWTECHNO[0] {
        return grid;
    }
    let Start::Beat(origin_beat_ordinal) = NEWTECHNO_PHRASE else {
        panic!("Newtechno phrase is named by its analysed beat");
    };
    let mut raw = grid.as_raw().clone();
    raw.meter = Some(kithara::beat::Meter {
        beats_per_bar: std::num::NonZeroU16::new(4).expect("fixture meter"),
        origin_beat_ordinal,
    });
    raw.downbeats = raw
        .beats
        .iter()
        .filter(|beat| (beat.ordinal - origin_beat_ordinal).rem_euclid(4) == 0)
        .map(|beat| kithara::beat::GridDownbeat {
            at: beat.at,
            beat_ordinal: beat.ordinal,
            confidence: None,
        })
        .collect();
    BeatGridModel::try_from(raw).expect("fixture phrase anchors a valid four-beat grid")
}

pub(crate) type PreparedSources = (Provider, TestServerHelper, Vec<String>);

pub(crate) async fn prepared_sources(provider: Provider) -> PreparedSources {
    let server = TestServerHelper::new().await;
    let paths = sources(provider, 4, &server).await;
    (provider, server, paths)
}
