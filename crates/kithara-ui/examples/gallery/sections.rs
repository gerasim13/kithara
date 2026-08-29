use super::fixture;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tab {
    Atoms,
    Buttons,
    Faders,
    Modules,
    Typography,
    Cells,
    Sizes,
    Tokens,
    Micro,
    Mixer,
    Vis,
    Chrome,
    Titlebars,
    Table,
    Tree,
    Library2,
    Stress,
    Menu,
    Clock,
    Pivot,
    Shader,
    Objects,
    Motion,
    Sprites,
    Lottie,
    Scene,
    TableLong,
    Custom,
    Skins,
}

impl Tab {
    pub(super) const ALL: [Self; 29] = [
        Self::Atoms,
        Self::Buttons,
        Self::Faders,
        Self::Modules,
        Self::Typography,
        Self::Cells,
        Self::Sizes,
        Self::Tokens,
        Self::Micro,
        Self::Mixer,
        Self::Vis,
        Self::Chrome,
        Self::Titlebars,
        Self::Table,
        Self::Tree,
        Self::Library2,
        Self::Stress,
        Self::Menu,
        Self::Clock,
        Self::Pivot,
        Self::Shader,
        Self::Objects,
        Self::Motion,
        Self::Sprites,
        Self::Lottie,
        Self::Scene,
        Self::TableLong,
        Self::Custom,
        Self::Skins,
    ];

    /// The page this tab shows, as the gallery's package names it: the role is
    /// the id the document states, and which file that role lives in is the
    /// manifest's to say.
    pub(super) fn entry(self) -> &'static str {
        fixture::document(match self {
            Self::Atoms => "gallery-atoms",
            Self::Buttons => "gallery-buttons",
            Self::Faders => "gallery-faders",
            Self::Modules => "gallery-modules",
            Self::Typography => "gallery-typography",
            Self::Cells => "gallery-cells",
            Self::Sizes => "gallery-sizes",
            Self::Tokens => "gallery-tokens",
            Self::Micro => "gallery-micro",
            Self::Mixer => "gallery-mixer",
            Self::Vis => "gallery-vis",
            Self::Chrome => "gallery-chrome",
            Self::Titlebars => "gallery-titlebars",
            Self::Table => "gallery-table",
            Self::Tree => "gallery-tree",
            Self::Library2 => "gallery-library2",
            Self::Stress => "gallery-stress",
            Self::Menu => "gallery-menu",
            Self::Clock => "gallery-clock",
            Self::Pivot => "gallery-pivot",
            Self::Shader => "gallery-shader",
            Self::Objects => "gallery-objects",
            Self::Motion => "gallery-motion",
            Self::Sprites => "gallery-sprites",
            Self::Lottie => "gallery-lottie",
            Self::Scene => "gallery-scene",
            Self::TableLong => "gallery-table-long",
            Self::Custom => "gallery-custom",
            Self::Skins => "gallery-skins",
        })
    }

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Atoms => 0,
            Self::Buttons => 1,
            Self::Faders => 2,
            Self::Modules => 3,
            Self::Typography => 4,
            Self::Cells => 5,
            Self::Sizes => 6,
            Self::Tokens => 7,
            Self::Micro => 8,
            Self::Mixer => 9,
            Self::Vis => 10,
            Self::Chrome => 11,
            Self::Titlebars => 12,
            Self::Table => 13,
            Self::Tree => 14,
            Self::Library2 => 15,
            Self::Stress => 16,
            Self::Menu => 17,
            Self::Clock => 18,
            Self::Pivot => 19,
            Self::Shader => 20,
            Self::Objects => 21,
            Self::Motion => 22,
            Self::Sprites => 23,
            Self::Lottie => 24,
            Self::Scene => 25,
            Self::TableLong => 26,
            Self::Custom => 27,
            Self::Skins => 28,
        }
    }
}

impl TryFrom<&str> for Tab {
    type Error = ();

    fn try_from(path: &str) -> Result<Self, ()> {
        let slug = path
            .strip_prefix("gallery/")
            .and_then(|rest| rest.strip_suffix("/item"))
            .ok_or(())?;
        match slug {
            "atoms" => Ok(Self::Atoms),
            "buttons" => Ok(Self::Buttons),
            "faders" => Ok(Self::Faders),
            "modules" => Ok(Self::Modules),
            "typography" => Ok(Self::Typography),
            "cells" => Ok(Self::Cells),
            "sizes" => Ok(Self::Sizes),
            "tokens" => Ok(Self::Tokens),
            "micro" => Ok(Self::Micro),
            "mixer" => Ok(Self::Mixer),
            "vis" => Ok(Self::Vis),
            "chrome" => Ok(Self::Chrome),
            "titlebars" => Ok(Self::Titlebars),
            "table" => Ok(Self::Table),
            "tree" => Ok(Self::Tree),
            "library2" => Ok(Self::Library2),
            "stress" => Ok(Self::Stress),
            "menu" => Ok(Self::Menu),
            "clock" => Ok(Self::Clock),
            "pivot" => Ok(Self::Pivot),
            "shader" => Ok(Self::Shader),
            "objects" => Ok(Self::Objects),
            "motion" => Ok(Self::Motion),
            "sprites" => Ok(Self::Sprites),
            "lottie" => Ok(Self::Lottie),
            "scene" => Ok(Self::Scene),
            "table_long" => Ok(Self::TableLong),
            "custom" => Ok(Self::Custom),
            "skins" => Ok(Self::Skins),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ModuleDemo {
    Deck,
    DeckMicro,
    GlobalBar,
    Telemetry,
    Layout,
}

impl ModuleDemo {
    pub(super) const ALL: [Self; 5] = [
        Self::Deck,
        Self::DeckMicro,
        Self::GlobalBar,
        Self::Telemetry,
        Self::Layout,
    ];

    pub(super) fn entry(self) -> &'static str {
        fixture::document(match self {
            Self::Deck => "gallery-modules",
            Self::DeckMicro => "gallery-modules-deck-micro",
            Self::GlobalBar => "gallery-modules-global-bar",
            Self::Telemetry => "gallery-modules-telemetry",
            Self::Layout => "gallery-modules-layout",
        })
    }

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Deck => 0,
            Self::DeckMicro => 1,
            Self::GlobalBar => 2,
            Self::Telemetry => 3,
            Self::Layout => 4,
        }
    }
}
