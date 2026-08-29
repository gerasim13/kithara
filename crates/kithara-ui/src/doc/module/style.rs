use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum IconName {
    Activity,
    Bell,
    Charts,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    ChevronsLeft,
    ChevronsRight,
    Circle,
    Clock,
    Collection,
    Crown,
    Disc,
    Faders,
    FastForward,
    Folder,
    FolderPlus,
    Gear,
    Headphones,
    Home,
    Instrument,
    Kithara,
    Lock,
    LockOpen,
    Maximize,
    Menu,
    Monitor,
    MusicNote,
    Orbit,
    Pause,
    Play,
    PlayReverse,
    Playlist,
    PlaylistAdd,
    Plus,
    Radio,
    RefreshCw,
    Repeat,
    RepeatOnce,
    Rewind,
    Save,
    Search,
    Shuffle,
    SkipBack,
    SkipForward,
    SlidersHorizontal,
    SpeakerHigh,
    SpeakerLow,
    SpeakerX,
    Usb,
    Waveform,
    X,
    ZoomIn,
    ZoomOut,
    Zvuk,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

/// The geometry a popover surface opens from.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum PopoverAt {
    #[default]
    Anchor,
    Pointer,
}

/// Which edge of the popover surface lines up with that geometry.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum PopoverAlign {
    #[default]
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum GlyphStyle {
    #[default]
    Default,
    Vis,
    Menu,
    MenuBurger,
    MenuSmall,
    MenuCell,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum DeckSummaryStyle {
    #[default]
    Default,
    Micro,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum WindowControlsStyle {
    #[default]
    Standard,
    Compact,
    CloseWide,
    CloseMicro,
    CloseFramed,
}

/// The typographic roles a document may name, written once and expanded
/// wherever the set has to appear again: the word a document writes, the entry
/// a skin gives it, and the lookup that joins the two.
macro_rules! text_roles {
    ($expand:ident) => {
        $expand! {
            #[default]
            body => Body,
            brand => Brand,
            brand_small => BrandSmall,
            caption => Caption,
            deck_letter => DeckLetter,
            micro_label => MicroLabel,
            mono => Mono,
            pivot_arrow => PivotArrow,
            pivot_duration => PivotDuration,
            pivot_footer => PivotFooter,
            pivot_label => PivotLabel,
            pivot_ratio => PivotRatio,
            pivot_small => PivotSmall,
            pivot_track_artist => PivotTrackArtist,
            pivot_track_title => PivotTrackTitle,
            pivot_title => PivotTitle,
            pivot_value => PivotValue,
            section => Section,
            telemetry => Telemetry,
            track_title => TrackTitle,
            vis_footer => VisFooter,
            vis_meta => VisMeta,
            vis_title => VisTitle,
        }
    };
}

pub(crate) use text_roles;

macro_rules! define_text_styles {
    ($($(#[$attr:meta])* $field:ident => $role:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
        #[non_exhaustive]
        pub enum TextStyle {
            $($(#[$attr])* $role,)*
        }
    };
}

text_roles!(define_text_styles);

impl TextStyle {
    /// The words this style sets, which are not always the words the document
    /// wrote: a micro label is small capitals, so it is set in capitals whatever
    /// case it was given.
    ///
    /// Every host asks here rather than deciding for itself, because the case a
    /// run is set in changes how wide it is, and two hosts that answered
    /// separately would lay the same document out differently.
    pub(crate) fn cased(self, content: String) -> String {
        match self {
            Self::MicroLabel => content.to_uppercase(),
            _ => content,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ButtonStyle {
    #[default]
    Default,
    Transport,
    TransportPrimary,
    MicroPrimary,
    VisNav,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ScalarFormat {
    #[default]
    Default,
    Percent,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum FaderStyle {
    #[default]
    Default,
    Volume,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum ChipStyle {
    #[default]
    Deck,
    PivotFamily,
    PivotMultiplier,
    Routing,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum WaveStyle {
    #[default]
    Default,
    Hero,
    Micro,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TableColumn {
    id: String,
    label: String,
    style: TableColumnStyle,
    width: f32,
    #[serde(default)]
    flexible: bool,
}

impl TableColumn {
    pub fn new<I, L>(id: I, label: L, style: TableColumnStyle, width: f32, flexible: bool) -> Self
    where
        I: Into<String>,
        L: Into<String>,
    {
        Self {
            id: id.into(),
            label: label.into(),
            style,
            width,
            flexible,
        }
    }

    #[must_use]
    pub fn flexible(&self) -> bool {
        self.flexible
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn style(&self) -> TableColumnStyle {
        self.style
    }

    #[must_use]
    pub fn width(&self) -> f32 {
        self.width
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum TableColumnStyle {
    Index,
    Badge,
    Primary,
    #[default]
    Secondary,
    Metric,
    Mono,
    Time,
    Meter,
    Transition,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Danger,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::TextStyle;

    #[kithara::test]
    fn a_micro_label_is_set_in_capitals() {
        assert_eq!(
            TextStyle::MicroLabel.cased("0.0.1-alpha4".to_owned()),
            "0.0.1-ALPHA4"
        );
    }

    #[kithara::test]
    fn every_other_style_keeps_the_case_the_document_wrote() {
        assert_eq!(
            TextStyle::Body.cased("0.0.1-alpha4".to_owned()),
            "0.0.1-alpha4"
        );
    }
}
