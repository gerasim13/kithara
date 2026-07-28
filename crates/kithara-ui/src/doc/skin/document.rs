use serde::{Deserialize, Serialize};

use super::{
    ButtonSkin, CellSkin, CheckboxSkin, ChipSkin, ChromeSkin, CrossfaderSkin, DeckSkin,
    DividerSkin, DragSkin, FaderSkin, GlobalBarSkin, KnobSkin, LayoutPreviewSkin, LayoutSkin,
    MeterSkin, NavSkin, PaletteDoc, ReadoutSkin, SegmentedSkin, SelectSkin, StatusDotSkin,
    SwatchSkin, TabLargeSkin, TelemetrySkin, TextInputSkin, TextSkin, ToggleSkin, TrackListSkin,
    TreeSkin, VisSkin, VuStereoSkin, VuVerticalSkin, WaveSkin, WindowSkin,
};
use crate::ids::DocId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SkinDoc {
    pub schema: String,
    pub version: u32,
    pub id: DocId,
    pub palette: PaletteDoc,
    pub layout: LayoutSkin,
    pub chrome: ChromeSkin,
    pub window: WindowSkin,
    pub text_input: TextInputSkin,
    pub knob: KnobSkin,
    pub crossfader: CrossfaderSkin,
    pub vu_stereo: VuStereoSkin,
    pub vu_vertical: VuVerticalSkin,
    pub vis: VisSkin,
    pub toggle: ToggleSkin,
    pub checkbox: CheckboxSkin,
    pub readout: ReadoutSkin,
    pub chip: ChipSkin,
    pub button: ButtonSkin,
    pub nav: NavSkin,
    pub tab_large: TabLargeSkin,
    pub text: TextSkin,
    pub segmented: SegmentedSkin,
    pub select: SelectSkin,
    pub status_dot: StatusDotSkin,
    pub swatch: SwatchSkin,
    pub cell: CellSkin,
    pub fader: FaderSkin,
    pub wave: WaveSkin,
    pub deck: DeckSkin,
    pub global_bar: GlobalBarSkin,
    pub divider: DividerSkin,
    pub drag: DragSkin,
    pub meter: MeterSkin,
    pub telemetry: TelemetrySkin,
    pub tree: TreeSkin,
    pub track_list: TrackListSkin,
    pub layout_preview: LayoutPreviewSkin,
}
