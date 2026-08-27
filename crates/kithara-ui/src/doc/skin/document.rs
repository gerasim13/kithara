use serde::{Deserialize, Serialize};

use super::{
    controls::{
        ButtonSkin, CellSkin, CheckboxSkin, ChipSkin, CrossfaderSkin, FaderSkin, KnobSkin,
        MenuSkin, NavSkin, PortalMapSkin, RangeSkin, ReadoutSkin, SegmentedSkin, SelectSkin,
        StatusDotSkin, SwatchSkin, TabLargeSkin, TextInputSkin, TextSkin, ToggleSkin, VisSkin,
        VuStereoSkin, VuVerticalSkin,
    },
    palette::PaletteDoc,
    panels::{
        DeckSkin, DividerSkin, DragSkin, GlobalBarSkin, LayoutPreviewSkin, MeterSkin, PopSkin,
        TableSkin, TelemetrySkin, TreeSkin, WaveSkin,
    },
    primitives::{ChromeSkin, LayoutSkin, ScrollSkin, WindowSkin},
};
use crate::{
    doc::ron_io,
    envelope::{self, DocKind},
    error::UiDocError,
    ids::{DocId, SourceUri},
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SkinDoc {
    pub button: ButtonSkin,
    pub cell: CellSkin,
    pub checkbox: CheckboxSkin,
    pub chip: ChipSkin,
    pub chrome: ChromeSkin,
    pub crossfader: CrossfaderSkin,
    pub deck: DeckSkin,
    pub divider: DividerSkin,
    pub id: DocId,
    pub drag: DragSkin,
    pub fader: FaderSkin,
    pub global_bar: GlobalBarSkin,
    pub knob: KnobSkin,
    pub layout_preview: LayoutPreviewSkin,
    pub layout: LayoutSkin,
    pub menu: MenuSkin,
    pub meter: MeterSkin,
    pub nav: NavSkin,
    pub palette: PaletteDoc,
    pub pop: PopSkin,
    pub portal_map: PortalMapSkin,
    pub range: RangeSkin,
    pub readout: ReadoutSkin,
    pub segmented: SegmentedSkin,
    pub select: SelectSkin,
    pub status_dot: StatusDotSkin,
    pub schema: String,
    pub scroll: ScrollSkin,
    pub swatch: SwatchSkin,
    pub tab_large: TabLargeSkin,
    pub telemetry: TelemetrySkin,
    pub text_input: TextInputSkin,
    pub text: TextSkin,
    pub toggle: ToggleSkin,
    pub table: TableSkin,
    pub tree: TreeSkin,
    pub vis: VisSkin,
    pub vu_stereo: VuStereoSkin,
    pub vu_vertical: VuVerticalSkin,
    pub wave: WaveSkin,
    pub window: WindowSkin,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum FontWeight {
    Normal,
    Medium,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum FontFamily {
    Display,
    Sans,
    Mono,
}

/// Parses and validates a complete skin document.
///
/// # Errors
/// Returns [`UiDocError`] when the envelope, body, or palette is invalid.
pub fn parse_skin(text: &str, origin: &SourceUri) -> Result<SkinDoc, UiDocError> {
    let envelope = envelope::probe(text, origin)?;
    if envelope.kind != DocKind::Skin {
        return Err(UiDocError::WrongDocKind {
            origin: origin.clone(),
            expected: DocKind::Skin.name(),
            found: envelope.kind.name(),
        });
    }
    let document: SkinDoc =
        ron_io::options()
            .from_str(text)
            .map_err(|source| UiDocError::Syntax {
                origin: origin.clone(),
                source: Box::new(source),
            })?;
    document.palette.validate(origin)?;
    Ok(document)
}
