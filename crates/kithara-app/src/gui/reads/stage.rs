use kithara::ui::render::{Node, PortalMapView, PortalTarget, ReadValue, Scope};
use num_traits::cast::AsPrimitive;

use super::value::Value;
use crate::gui::ui::cache::StageView;

mod consts {
    pub(super) const BPM_CEILING: f32 = 200.0;
    pub(super) const BPM_FLOOR: f32 = 60.0;
}

/// One deck as the tempo map sees it.
#[derive(Clone, Copy)]
pub(super) struct DeckTempo {
    pub(super) bpm: Option<f32>,
    pub(super) focused: bool,
    pub(super) position: f64,
}

/// The tempo axis: the decks' analysed BPMs across the BPM span the map draws.
pub(super) struct TempoNode {
    targets: Vec<PortalTarget>,
    master: f32,
}

impl TempoNode {
    pub(super) fn new(decks: &[DeckTempo]) -> Self {
        let master = focused(decks).and_then(|deck| deck.bpm).unwrap_or_default();
        let targets = decks
            .iter()
            .filter_map(|deck| {
                Some(PortalTarget {
                    bpm: deck.bpm?,
                    is_selected: deck.focused,
                })
            })
            .collect();
        Self { targets, master }
    }
}

impl<'a> Node<'a> for &'a TempoNode {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "map" => ReadValue::PortalMap(PortalMapView {
                min: consts::BPM_FLOOR,
                max: consts::BPM_CEILING,
                master: self.master,
                targets: &self.targets,
            }),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

/// What the visualiser reads besides the master level it takes from the player:
/// the preset the host holds, and a clock. The clock is the focused deck's
/// playhead, so the surface moves with playback rather than with wall time.
#[derive(Clone, Copy)]
pub(super) struct VisNode<'a> {
    view: &'a StageView,
    clock: f64,
}

impl<'a> VisNode<'a> {
    pub(super) fn new(view: &'a StageView, decks: &[DeckTempo]) -> Self {
        Self {
            view,
            clock: focused(decks).map_or(0.0, |deck| deck.position),
        }
    }
}

impl<'a> Node<'a> for VisNode<'a> {
    fn child(&self, segment: &str, _scope: Scope<'_>) -> Option<Box<dyn Node<'a> + 'a>> {
        let value = match segment {
            "preset" => ReadValue::Scalar(self.view.preset.as_()),
            "time" => ReadValue::Scalar(self.clock),
            _ => return None,
        };
        Some(Box::new(Value(value)))
    }
}

fn focused(decks: &[DeckTempo]) -> Option<&DeckTempo> {
    decks.iter().find(|deck| deck.focused)
}
