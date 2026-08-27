use std::sync::LazyLock;

#[cfg(feature = "render")]
use crate::render::Skin;
use crate::{
    ids::SourceUri,
    skin::{SkinDoc, load_skin},
    source::{Limits, MemResolver},
    text::{TextDoc, parse_text},
};

pub const MICRO_PRESET: &str = "micro.klayout.ron";
pub const PLAYER_PRESET: &str = "player.klayout.ron";
pub const DARK_SKIN: &str = include_str!("../assets/kithara-dark.kskin.ron");
pub const DARK_SKIN_PATH: &str = "kithara-dark.kskin.ron";
/// Paper and neon: two skins written over the dark one, restating its palette
/// and nothing else.
pub const LIGHT_SKIN: &str = include_str!("../assets/kithara-light.kskin.ron");
pub const LIGHT_SKIN_PATH: &str = "kithara-light.kskin.ron";
pub const NEON_SKIN: &str = include_str!("../assets/kithara-neon.kskin.ron");
pub const NEON_SKIN_PATH: &str = "kithara-neon.kskin.ron";
/// Every skin this crate ships, in the order a picker offers them. The first
/// is the one a host wears when it names none.
pub const SKIN_PATHS: [&str; 3] = [DARK_SKIN_PATH, LIGHT_SKIN_PATH, NEON_SKIN_PATH];
/// Eight frames of a growing arc, in one row, for the sprite page and its
/// cross-host proof.
pub const SPINNER_SHEET: &[u8] = include_bytes!("../assets/sprites/spinner.png");
pub const TEXT_EN: &str = include_str!("../assets/kithara-en.ktext.ron");

#[must_use]
pub fn resolver() -> MemResolver {
    const ASSETS: &[(&str, &str)] = &[
        (DARK_SKIN_PATH, DARK_SKIN),
        (LIGHT_SKIN_PATH, LIGHT_SKIN),
        (NEON_SKIN_PATH, NEON_SKIN),
        (MICRO_PRESET, include_str!("../assets/micro.klayout.ron")),
        (PLAYER_PRESET, include_str!("../assets/player.klayout.ron")),
        (
            "modules/deck-micro.kmodule.ron",
            include_str!("../assets/modules/deck-micro.kmodule.ron"),
        ),
        (
            "modules/deck-micro/bar.kmodule.ron",
            include_str!("../assets/modules/deck-micro/bar.kmodule.ron"),
        ),
        (
            "modules/app-menu.kmodule.ron",
            include_str!("../assets/modules/app-menu.kmodule.ron"),
        ),
        (
            "modules/app-menu/hint-row.kmodule.ron",
            include_str!("../assets/modules/app-menu/hint-row.kmodule.ron"),
        ),
        (
            "modules/app-menu/layout-row.kmodule.ron",
            include_str!("../assets/modules/app-menu/layout-row.kmodule.ron"),
        ),
        (
            "modules/app-menu/module-cell.kmodule.ron",
            include_str!("../assets/modules/app-menu/module-cell.kmodule.ron"),
        ),
        (
            "modules/app-menu/toggle-row.kmodule.ron",
            include_str!("../assets/modules/app-menu/toggle-row.kmodule.ron"),
        ),
        (
            "modules/app-menu/window-row.kmodule.ron",
            include_str!("../assets/modules/app-menu/window-row.kmodule.ron"),
        ),
        (
            "modules/master-clock.kmodule.ron",
            include_str!("../assets/modules/master-clock.kmodule.ron"),
        ),
        (
            "modules/master-clock/surface.kmodule.ron",
            include_str!("../assets/modules/master-clock/surface.kmodule.ron"),
        ),
        (
            "modules/master-clock/source-row.kmodule.ron",
            include_str!("../assets/modules/master-clock/source-row.kmodule.ron"),
        ),
        (
            "modules/global-bar.kmodule.ron",
            include_str!("../assets/modules/global-bar.kmodule.ron"),
        ),
        (
            "modules/deck.kmodule.ron",
            include_str!("../assets/modules/deck.kmodule.ron"),
        ),
        (
            "modules/deck/transport.kmodule.ron",
            include_str!("../assets/modules/deck/transport.kmodule.ron"),
        ),
        (
            "modules/deck/quality.kmodule.ron",
            include_str!("../assets/modules/deck/quality.kmodule.ron"),
        ),
        (
            "modules/deck/quality/auto.kmodule.ron",
            include_str!("../assets/modules/deck/quality/auto.kmodule.ron"),
        ),
        (
            "modules/deck/quality/row.kmodule.ron",
            include_str!("../assets/modules/deck/quality/row.kmodule.ron"),
        ),
        (
            "modules/library.kmodule.ron",
            include_str!("../assets/modules/library.kmodule.ron"),
        ),
    ];
    let mut resolver = MemResolver::default();
    for (path, text) in ASSETS {
        resolver.insert(path, text);
    }
    resolver
}

#[must_use]
pub fn skin_doc() -> &'static SkinDoc {
    static SKIN_DOC: LazyLock<SkinDoc> = LazyLock::new(|| {
        load_skin(&resolver(), DARK_SKIN_PATH, &Limits::default())
            .unwrap_or_else(|error| panic!("embedded kithara dark skin must be valid: {error}"))
    });
    &SKIN_DOC
}

#[must_use]
pub fn text_doc() -> &'static TextDoc {
    static TEXT_DOC: LazyLock<TextDoc> = LazyLock::new(|| {
        parse_text(TEXT_EN, &text_origin())
            .unwrap_or_else(|error| panic!("embedded kithara text catalog must be valid: {error}"))
    });
    &TEXT_DOC
}

/// The skin a host wears when it names none.
#[cfg(feature = "render")]
#[must_use]
pub fn skin() -> &'static Skin {
    &skins()[0]
}

/// Every shipped skin, resolved, in the order [`SKIN_PATHS`] declares.
#[cfg(feature = "render")]
#[must_use]
pub fn skins() -> &'static [Skin] {
    static SKINS: LazyLock<Vec<Skin>> = LazyLock::new(|| {
        let resolver = resolver();
        SKIN_PATHS
            .iter()
            .map(|path| {
                let origin = SourceUri((*path).to_owned());
                let document = load_skin(&resolver, path, &Limits::default())
                    .unwrap_or_else(|error| panic!("embedded skin {path} must be valid: {error}"));
                Skin::resolve(document, text_doc(), &origin)
                    .unwrap_or_else(|error| panic!("embedded skin {path} must resolve: {error}"))
            })
            .collect()
    });
    &SKINS
}

fn text_origin() -> SourceUri {
    SourceUri("builtin:kithara-en.ktext.ron".to_owned())
}
