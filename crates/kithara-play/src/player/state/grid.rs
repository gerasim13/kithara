use std::num::NonZeroU32;

use kithara_platform::sync::{Arc, Mutex};
use kithara_warp::{
    AssetAxis, AssetExtent, BeatGrid, BeatGridId, BeatGridModelError, BeatGridRevision,
    BeatGridSnapshot, MapAxis,
};
use tracing::warn;

use crate::resource::PreparedGrid;

/// The beat geometry one player publishes for the track it holds.
///
/// Identity belongs to the player, not to the track: the group attaches this
/// member once and keeps it, and every load it plays states its geometry
/// through the same identity at a later revision. That is what lets a track
/// arrive, be replaced, or be released without a topology change.
///
/// The model itself is never rewritten. It states its beats in media seconds,
/// and this is the only place they become frames, against the axis the player
/// currently decodes onto. A rate change therefore reprojects the model it
/// already holds instead of rescaling a result that was computed once.
#[derive(Clone)]
pub(crate) struct TrackGrid(Arc<Owner>);

struct Owner {
    id: BeatGridId,
    held: Mutex<Held>,
}

struct Held {
    axis: AssetAxis,
    /// The loaded track's slot, and the generation of it already published.
    /// `None` between loads, which is what makes the grid unavailable.
    source: Option<(Arc<PreparedGrid>, u64)>,
    published: BeatGridSnapshot,
}

impl TrackGrid {
    /// A player's grid before it has loaded anything.
    ///
    /// `sample_rate` is the axis the player decodes onto; the extent is
    /// unknown until a track states one, which leaves positions past the
    /// geometry uncovered rather than outside the domain.
    pub(crate) fn new(id: BeatGridId, sample_rate: NonZeroU32) -> Self {
        let axis = AssetAxis::new(sample_rate, AssetExtent::Unknown);
        Self(Arc::new(Owner {
            id,
            held: Mutex::new(Held {
                axis,
                source: None,
                published: unavailable(id, BeatGridRevision::first(), axis),
            }),
        }))
    }

    /// Follow the prepared grid of the track now loaded: `sample_rate` is the
    /// axis this player decodes onto, and `frames` the length of the load when
    /// it states one.
    pub(crate) fn load(
        &self,
        prepared: &Arc<PreparedGrid>,
        sample_rate: NonZeroU32,
        frames: Option<u64>,
    ) {
        let mut held = self.0.held.lock();
        held.axis = AssetAxis::new(
            sample_rate,
            frames.map_or(AssetExtent::Unknown, AssetExtent::Bounded),
        );
        held.source = Some((Arc::clone(prepared), 0));
        self.0.refresh(&mut held);
        drop(held);
    }

    /// Let go of the loaded track's geometry.
    pub(crate) fn release(&self) {
        let mut held = self.0.held.lock();
        held.source = None;
        self.0.refresh(&mut held);
        drop(held);
    }
}

impl Owner {
    /// Rebuild what this grid publishes from what it now holds.
    ///
    /// A model that cannot be expressed on the decoded axis leaves the grid
    /// unavailable at a later revision: a geometry error is a statement about
    /// this track, not a reason to keep publishing the previous one.
    fn refresh(&self, held: &mut Held) {
        let Some(revision) = held.published.revision().checked_next() else {
            warn!(grid_id = ?self.id, "track grid: the revision space is exhausted");
            return;
        };
        held.published = match self.materialize(held, revision) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(grid_id = ?self.id, %error, "track grid: the model has no geometry on this axis");
                unavailable(self.id, revision, held.axis)
            }
        };
    }

    fn materialize(
        &self,
        held: &mut Held,
        revision: BeatGridRevision,
    ) -> Result<BeatGridSnapshot, BeatGridModelError> {
        let Some((prepared, published)) = held.source.as_mut() else {
            return Ok(unavailable(self.id, revision, held.axis));
        };
        let (generation, model) = prepared.read();
        *published = generation;
        model.map_or_else(
            || Ok(unavailable(self.id, revision, held.axis)),
            |model| BeatGridSnapshot::model(self.id, revision, &model, held.axis),
        )
    }
}

fn unavailable(id: BeatGridId, revision: BeatGridRevision, axis: AssetAxis) -> BeatGridSnapshot {
    BeatGridSnapshot::unavailable(id, revision, MapAxis::Asset(axis))
}

impl BeatGrid for TrackGrid {
    fn id(&self) -> BeatGridId {
        self.0.id
    }

    /// One immutable observation of this player's track geometry.
    ///
    /// A read is where a source that answered late takes effect: the slot
    /// states its own generation, so the grid is rebuilt exactly once per
    /// answer and an unchanged slot hands back the revision it already has.
    fn snapshot(&self) -> BeatGridSnapshot {
        let mut held = self.0.held.lock();
        let stale = held
            .source
            .as_ref()
            .is_some_and(|(prepared, published)| prepared.read().0 != *published);
        if stale {
            self.0.refresh(&mut held);
        }
        held.published.clone()
    }
}

#[cfg(test)]
mod tests {
    use kithara_beat::{
        BeatGridModel, BeatGridState as WireState, GridBeat, RawBeatGrid, SCHEMA_VERSION,
    };
    use kithara_test_utils::kithara;
    use kithara_warp::{AssetFrame, Beat, BeatGridQuery, BeatGridState, MapPoint, MapPosition};

    use super::*;

    fn rate() -> NonZeroU32 {
        NonZeroU32::new(48_000).expect("48000 is not zero")
    }

    /// A grid stating two beats a half second apart, over two media seconds.
    fn served() -> Arc<BeatGridModel> {
        Arc::new(
            BeatGridModel::try_from(RawBeatGrid {
                schema_version: SCHEMA_VERSION,
                model_id: "served".to_owned(),
                revision: 1,
                state: WireState::Final,
                duration: Some(2.0),
                bpm: 120.0,
                beats: vec![
                    GridBeat {
                        at: 0.0,
                        ordinal: 0,
                        confidence: Some(0.9),
                    },
                    GridBeat {
                        at: 0.5,
                        ordinal: 1,
                        confidence: Some(0.9),
                    },
                ],
                downbeats: Vec::new(),
                meter: None,
            })
            .expect("the fixture grid holds together"),
        )
    }

    fn grid() -> TrackGrid {
        TrackGrid::new(
            BeatGridId::allocate().expect("the identity space is available"),
            rate(),
        )
    }

    /// Where the snapshot places `ordinal` on its own axis.
    fn position_of(snapshot: &BeatGridSnapshot, ordinal: f64) -> MapPosition {
        let beat = MapPoint::new(
            snapshot.stamp(),
            Beat::new(ordinal).expect("a finite beat ordinal"),
        );
        let BeatGridQuery::Resolved(estimate) = snapshot.position_at(beat) else {
            panic!("the grid must place beat {ordinal}");
        };
        *estimate.value().value()
    }

    /// The asset frame a decoded axis names.
    fn frame(frames: f64) -> MapPosition {
        MapPosition::Asset(AssetFrame::new(frames).expect("a finite fixture frame"))
    }

    #[kithara::test]
    fn a_player_publishes_no_geometry_before_it_loads_anything() {
        let snapshot = grid().snapshot();

        assert!(
            matches!(snapshot.state(), BeatGridState::Unavailable(_)),
            "an empty player states no geometry: {:?}",
            snapshot.state()
        );
        assert!(
            matches!(snapshot.axis(), MapAxis::Asset(_)),
            "a track grid is asset-native even before it holds a track"
        );
    }

    #[kithara::test]
    fn a_supplied_grid_reaches_the_decoded_axis_in_media_seconds() {
        let track = TrackGrid::new(
            BeatGridId::allocate().expect("the identity space is available"),
            rate(),
        );
        let prepared = Arc::new(PreparedGrid::holding(served()));

        track.load(&prepared, rate(), Some(96_000));

        let snapshot = track.snapshot();
        assert_eq!(
            position_of(&snapshot, 0.0),
            frame(0.0),
            "beat zero sits at the start of the decoded asset"
        );
        assert_eq!(
            position_of(&snapshot, 1.0),
            frame(24_000.0),
            "a beat at 0.5 media seconds is frame 24000 of a 48 kHz decode, \
             not a fraction of the host's output rate"
        );
    }

    /// The same model, decoded twice at different rates, must place its beats
    /// at each axis's own frames: the model is reprojected, never rescaled.
    #[kithara::test]
    fn a_rate_change_reprojects_the_model_it_already_holds() {
        let track = grid();
        let prepared = Arc::new(PreparedGrid::holding(served()));

        track.load(&prepared, rate(), Some(96_000));
        let first = track.snapshot();
        let doubled = NonZeroU32::new(96_000).expect("96000 is not zero");
        track.load(&prepared, doubled, Some(192_000));
        let second = track.snapshot();

        assert_eq!(
            position_of(&first, 1.0),
            frame(24_000.0),
            "48 kHz places the second beat at 24000"
        );
        assert_eq!(
            position_of(&second, 1.0),
            frame(48_000.0),
            "96 kHz places the very same beat at 48000"
        );
        assert!(
            second.revision() > first.revision(),
            "a later geometry is a later revision"
        );
    }

    #[kithara::test]
    fn a_source_that_answers_late_reaches_the_load_that_asked() {
        let track = grid();
        let prepared = Arc::new(PreparedGrid::default());

        track.load(&prepared, rate(), Some(96_000));
        let pending = track.snapshot();
        prepared.put(served());
        let answered = track.snapshot();

        assert!(
            matches!(pending.state(), BeatGridState::Unavailable(_)),
            "a load still reading its source publishes no geometry"
        );
        assert_eq!(
            position_of(&answered, 1.0),
            frame(24_000.0),
            "the answer takes effect on the next observation"
        );
        assert!(
            answered.revision() > pending.revision(),
            "the answer is a later revision of the same grid"
        );
        assert_eq!(
            answered.id(),
            pending.id(),
            "the identity is the player's and does not move"
        );
    }

    #[kithara::test]
    fn an_unchanged_slot_does_not_move_the_revision() {
        let track = grid();
        let prepared = Arc::new(PreparedGrid::holding(served()));

        track.load(&prepared, rate(), Some(96_000));
        let first = track.snapshot();
        let again = track.snapshot();

        assert_eq!(
            first.revision(),
            again.revision(),
            "nothing changed, so nothing was republished"
        );
    }

    #[kithara::test]
    fn releasing_a_track_leaves_the_grid_unavailable() {
        let track = grid();
        let prepared = Arc::new(PreparedGrid::holding(served()));

        track.load(&prepared, rate(), Some(96_000));
        let loaded = track.snapshot();
        track.release();
        let released = track.snapshot();

        assert!(
            matches!(released.state(), BeatGridState::Unavailable(_)),
            "a player holding nothing states no geometry: {:?}",
            released.state()
        );
        assert!(
            released.revision() > loaded.revision(),
            "letting go is a later revision, never a rewind"
        );
    }
}
