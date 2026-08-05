use super::{Visit, badge, bar, deck, label, panel, press, scalar, switch, window};
use crate::expand::ControlSpec;

/// Applies one operation to whichever built-in control the document named.
///
/// The single place that maps a document variant to the file that owns it.
/// Size, layout and both hosts all ride through here, so teaching the toolkit a
/// control is one file and one arm rather than an edit in every table that has
/// ever met one.
pub(crate) fn dispatch<V: Visit>(spec: &ControlSpec, visit: V) -> V::Output {
    match spec {
        ControlSpec::DeckSummary { .. } => visit.visit(&deck::Summary),
        ControlSpec::Brand => visit.visit(&bar::Brand),
        ControlSpec::Spacer => visit.visit(&bar::Spacer),
        ControlSpec::Divider => visit.visit(&bar::Divider),
        ControlSpec::PresetSelector => visit.visit(&bar::Preset),
        ControlSpec::SettingsButton => visit.visit(&press::Settings),
        ControlSpec::WindowDrag => visit.visit(&window::Drag),
        ControlSpec::TitleBar { .. } => visit.visit(&window::TitleBar),
        ControlSpec::WindowControls { style } => visit.visit(&window::Controls { style: *style }),
        ControlSpec::Text { style, .. } => visit.visit(&label::Text { style: *style }),
        ControlSpec::Glyph { style, .. } => visit.visit(&label::Glyph { style: *style }),
        ControlSpec::NavItem { .. } => visit.visit(&press::NavItem),
        ControlSpec::TabLarge { .. } => visit.visit(&press::Tab),
        ControlSpec::Button { style, .. } => visit.visit(&press::Button { style: *style }),
        ControlSpec::Bpm { .. } => visit.visit(&deck::Bpm),
        ControlSpec::Time => visit.visit(&deck::Time),
        ControlSpec::Scalar { .. } => visit.visit(&label::Telemetry),
        ControlSpec::Crossfader { .. } => visit.visit(&scalar::Crossfader),
        ControlSpec::Fader { .. } => visit.visit(&scalar::Fader),
        ControlSpec::Wave { .. } => visit.visit(&deck::Wave),
        ControlSpec::Vis => visit.visit(&deck::Vis),
        ControlSpec::TrackList { .. } => visit.visit(&panel::TrackList),
        ControlSpec::Tree { .. } => visit.visit(&panel::Tree),
        ControlSpec::ContextBar { .. } => visit.visit(&panel::ContextBar),
        ControlSpec::Toggle => visit.visit(&switch::Toggle),
        ControlSpec::Checkbox => visit.visit(&switch::Checkbox),
        ControlSpec::Segmented { .. } => visit.visit(&press::Segmented),
        ControlSpec::Select { .. } => visit.visit(&label::Select),
        ControlSpec::StatusDot { .. } => visit.visit(&badge::StatusDot),
        ControlSpec::Swatch { .. } => visit.visit(&badge::Swatch),
        ControlSpec::Cell { .. } => visit.visit(&badge::Cell),
        ControlSpec::Readout { .. } => visit.visit(&label::Readout),
        ControlSpec::Chip { .. } => visit.visit(&press::Chip),
        ControlSpec::Knob { .. } => visit.visit(&scalar::Knob),
        ControlSpec::Meter => visit.visit(&scalar::Meter),
        ControlSpec::VuStereo => visit.visit(&scalar::VuStereo),
        ControlSpec::VuVertical { .. } => visit.visit(&scalar::VuVertical),
    }
}
