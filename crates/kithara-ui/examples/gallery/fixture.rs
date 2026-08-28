//! The gallery's documents and their resolver, apart from the window that shows
//! them. A measurement harness mounts the same pages from here without pulling
//! in a toolkit main.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use kithara_ui::{
    builtin,
    ids::ScreenRole,
    package::load_package,
    source::{FileResolver, MemResolver, OverlayResolver},
};

pub(crate) struct Consts;

impl Consts {
    pub(crate) const HEIGHT: f32 = 720.0;
    /// The smallest window the gallery opens to, so a page can be dragged
    /// down to the room its adaptive and revealed cells answer.
    pub(crate) const MIN_HEIGHT: f32 = 320.0;
    pub(crate) const MIN_WIDTH: f32 = 400.0;
    pub(crate) const STRESS_TICK_MS: u64 = 16;
    pub(crate) const WIDTH: f32 = 1300.0;
}

const ASSETS: &[(&str, &str)] = &[
    (
        "package.kpackage.ron",
        include_str!("assets/package.kpackage.ron"),
    ),
    (
        "gallery-clock.klayout.ron",
        include_str!("assets/gallery-clock.klayout.ron"),
    ),
    (
        "gallery-skins.klayout.ron",
        include_str!("assets/gallery-skins.klayout.ron"),
    ),
    (
        "gallery-atoms.klayout.ron",
        include_str!("assets/gallery-atoms.klayout.ron"),
    ),
    (
        "gallery-buttons.klayout.ron",
        include_str!("assets/gallery-buttons.klayout.ron"),
    ),
    (
        "gallery-cells.klayout.ron",
        include_str!("assets/gallery-cells.klayout.ron"),
    ),
    (
        "gallery-chrome.klayout.ron",
        include_str!("assets/gallery-chrome.klayout.ron"),
    ),
    (
        "gallery-faders.klayout.ron",
        include_str!("assets/gallery-faders.klayout.ron"),
    ),
    (
        "gallery-library2.klayout.ron",
        include_str!("assets/gallery-library2.klayout.ron"),
    ),
    (
        "gallery-menu.klayout.ron",
        include_str!("assets/gallery-menu.klayout.ron"),
    ),
    (
        "gallery-pivot.klayout.ron",
        include_str!("assets/gallery-pivot.klayout.ron"),
    ),
    (
        "gallery-shader.klayout.ron",
        include_str!("assets/gallery-shader.klayout.ron"),
    ),
    (
        "gallery-custom.klayout.ron",
        include_str!("assets/gallery-custom.klayout.ron"),
    ),
    (
        "gallery-objects.klayout.ron",
        include_str!("assets/gallery-objects.klayout.ron"),
    ),
    (
        "modules/tabs/objects.kmodule.ron",
        include_str!("assets/modules/tabs/objects.kmodule.ron"),
    ),
    (
        "gallery-motion.klayout.ron",
        include_str!("assets/gallery-motion.klayout.ron"),
    ),
    (
        "modules/tabs/motion.kmodule.ron",
        include_str!("assets/modules/tabs/motion.kmodule.ron"),
    ),
    (
        "gallery-sprites.klayout.ron",
        include_str!("assets/gallery-sprites.klayout.ron"),
    ),
    (
        "modules/tabs/sprites.kmodule.ron",
        include_str!("assets/modules/tabs/sprites.kmodule.ron"),
    ),
    (
        "gallery-lottie.klayout.ron",
        include_str!("assets/gallery-lottie.klayout.ron"),
    ),
    (
        "modules/tabs/lottie.kmodule.ron",
        include_str!("assets/modules/tabs/lottie.kmodule.ron"),
    ),
    (
        "modules/tabs/shader.kmodule.ron",
        include_str!("assets/modules/tabs/shader.kmodule.ron"),
    ),
    (
        "modules/tabs/custom.kmodule.ron",
        include_str!("assets/modules/tabs/custom.kmodule.ron"),
    ),
    (
        "modules/tabs/field.wgsl",
        include_str!("assets/modules/tabs/field.wgsl"),
    ),
    (
        "gallery-micro.klayout.ron",
        include_str!("assets/gallery-micro.klayout.ron"),
    ),
    (
        "gallery-mixer.klayout.ron",
        include_str!("assets/gallery-mixer.klayout.ron"),
    ),
    (
        "gallery-modules-deck-micro.klayout.ron",
        include_str!("assets/gallery-modules-deck-micro.klayout.ron"),
    ),
    (
        "gallery-modules-global-bar.klayout.ron",
        include_str!("assets/gallery-modules-global-bar.klayout.ron"),
    ),
    (
        "gallery-modules-layout.klayout.ron",
        include_str!("assets/gallery-modules-layout.klayout.ron"),
    ),
    (
        "gallery-modules-telemetry.klayout.ron",
        include_str!("assets/gallery-modules-telemetry.klayout.ron"),
    ),
    (
        "gallery-modules.klayout.ron",
        include_str!("assets/gallery-modules.klayout.ron"),
    ),
    (
        "gallery-sizes.klayout.ron",
        include_str!("assets/gallery-sizes.klayout.ron"),
    ),
    (
        "gallery-stress.klayout.ron",
        include_str!("assets/gallery-stress.klayout.ron"),
    ),
    (
        "gallery-titlebars.klayout.ron",
        include_str!("assets/gallery-titlebars.klayout.ron"),
    ),
    (
        "gallery-tokens.klayout.ron",
        include_str!("assets/gallery-tokens.klayout.ron"),
    ),
    (
        "gallery-table.klayout.ron",
        include_str!("assets/gallery-table.klayout.ron"),
    ),
    (
        "gallery-table-long.klayout.ron",
        include_str!("assets/gallery-table-long.klayout.ron"),
    ),
    (
        "gallery-tree.klayout.ron",
        include_str!("assets/gallery-tree.klayout.ron"),
    ),
    (
        "gallery-typography.klayout.ron",
        include_str!("assets/gallery-typography.klayout.ron"),
    ),
    (
        "gallery-vis.klayout.ron",
        include_str!("assets/gallery-vis.klayout.ron"),
    ),
    (
        "modules/app-menu.kmodule.ron",
        include_str!("../../assets/modules/app-menu.kmodule.ron"),
    ),
    (
        "modules/app-menu/hint-row.kmodule.ron",
        include_str!("../../assets/modules/app-menu/hint-row.kmodule.ron"),
    ),
    (
        "modules/app-menu/layout-row.kmodule.ron",
        include_str!("../../assets/modules/app-menu/layout-row.kmodule.ron"),
    ),
    (
        "modules/app-menu/module-cell.kmodule.ron",
        include_str!("../../assets/modules/app-menu/module-cell.kmodule.ron"),
    ),
    (
        "modules/app-menu/toggle-row.kmodule.ron",
        include_str!("../../assets/modules/app-menu/toggle-row.kmodule.ron"),
    ),
    (
        "modules/app-menu/window-row.kmodule.ron",
        include_str!("../../assets/modules/app-menu/window-row.kmodule.ron"),
    ),
    (
        "modules/deck/key-lock.kmodule.ron",
        include_str!("../../assets/modules/deck/key-lock.kmodule.ron"),
    ),
    (
        "modules/deck/overview-row.kmodule.ron",
        include_str!("../../assets/modules/deck/overview-row.kmodule.ron"),
    ),
    (
        "modules/master-clock.kmodule.ron",
        include_str!("../../assets/modules/master-clock.kmodule.ron"),
    ),
    (
        "modules/master-clock/source-row.kmodule.ron",
        include_str!("../../assets/modules/master-clock/source-row.kmodule.ron"),
    ),
    (
        "modules/pivot-portals.kmodule.ron",
        include_str!("../../assets/modules/pivot-portals.kmodule.ron"),
    ),
    (
        "modules/pivot-portals/row.kmodule.ron",
        include_str!("../../assets/modules/pivot-portals/row.kmodule.ron"),
    ),
    (
        "modules/pivot-portals/track-row.kmodule.ron",
        include_str!("../../assets/modules/pivot-portals/track-row.kmodule.ron"),
    ),
    (
        "modules/module-deck-micro.kmodule.ron",
        include_str!("assets/modules/module-deck-micro.kmodule.ron"),
    ),
    (
        "modules/module-deck.kmodule.ron",
        include_str!("assets/modules/module-deck.kmodule.ron"),
    ),
    (
        "modules/module-global-bar.kmodule.ron",
        include_str!("assets/modules/module-global-bar.kmodule.ron"),
    ),
    (
        "modules/module-layout.kmodule.ron",
        include_str!("assets/modules/module-layout.kmodule.ron"),
    ),
    (
        "modules/module-tabs.kmodule.ron",
        include_str!("assets/modules/module-tabs.kmodule.ron"),
    ),
    (
        "modules/module-telemetry.kmodule.ron",
        include_str!("assets/modules/module-telemetry.kmodule.ron"),
    ),
    (
        "modules/nav.kmodule.ron",
        include_str!("assets/modules/nav.kmodule.ron"),
    ),
    (
        "modules/nav/item.kmodule.ron",
        include_str!("assets/modules/nav/item.kmodule.ron"),
    ),
    (
        "modules/primitives/chips.kmodule.ron",
        include_str!("assets/modules/primitives/chips.kmodule.ron"),
    ),
    (
        "modules/primitives/knobs.kmodule.ron",
        include_str!("assets/modules/primitives/knobs.kmodule.ron"),
    ),
    (
        "modules/primitives/meters.kmodule.ron",
        include_str!("assets/modules/primitives/meters.kmodule.ron"),
    ),
    (
        "modules/primitives/readouts.kmodule.ron",
        include_str!("assets/modules/primitives/readouts.kmodule.ron"),
    ),
    (
        "modules/primitives/toggles.kmodule.ron",
        include_str!("assets/modules/primitives/toggles.kmodule.ron"),
    ),
    (
        "modules/stress.kmodule.ron",
        include_str!("assets/modules/stress.kmodule.ron"),
    ),
    (
        "modules/tabs/atoms.kmodule.ron",
        include_str!("assets/modules/tabs/atoms.kmodule.ron"),
    ),
    (
        "modules/tabs/buttons.kmodule.ron",
        include_str!("assets/modules/tabs/buttons.kmodule.ron"),
    ),
    (
        "modules/tabs/cells.kmodule.ron",
        include_str!("assets/modules/tabs/cells.kmodule.ron"),
    ),
    (
        "modules/tabs/clock.kmodule.ron",
        include_str!("assets/modules/tabs/clock.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-full-all.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-full-all.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-join-left.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-join-left.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-join-right.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-join-right.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-open-top.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-open-top.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-row-a.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-row-a.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-row-b.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-row-b.kmodule.ron"),
    ),
    (
        "modules/tabs/chrome-row-c.kmodule.ron",
        include_str!("assets/modules/tabs/chrome-row-c.kmodule.ron"),
    ),
    (
        "modules/tabs/faders.kmodule.ron",
        include_str!("assets/modules/tabs/faders.kmodule.ron"),
    ),
    (
        "modules/tabs/library2.kmodule.ron",
        include_str!("assets/modules/tabs/library2.kmodule.ron"),
    ),
    (
        "modules/tabs/menu-context.kmodule.ron",
        include_str!("assets/modules/tabs/menu-context.kmodule.ron"),
    ),
    (
        "modules/tabs/menu-context/track-row.kmodule.ron",
        include_str!("assets/modules/tabs/menu-context/track-row.kmodule.ron"),
    ),
    (
        "modules/tabs/menu-notes.kmodule.ron",
        include_str!("assets/modules/tabs/menu-notes.kmodule.ron"),
    ),
    (
        "modules/tabs/micro-notes.kmodule.ron",
        include_str!("assets/modules/tabs/micro-notes.kmodule.ron"),
    ),
    (
        "modules/tabs/mixer-1d.kmodule.ron",
        include_str!("assets/modules/tabs/mixer-1d.kmodule.ron"),
    ),
    (
        "modules/tabs/mixer-1g.kmodule.ron",
        include_str!("assets/modules/tabs/mixer-1g.kmodule.ron"),
    ),
    (
        "modules/tabs/mixer-label.kmodule.ron",
        include_str!("assets/modules/tabs/mixer-label.kmodule.ron"),
    ),
    (
        "modules/tabs/sizes.kmodule.ron",
        include_str!("assets/modules/tabs/sizes.kmodule.ron"),
    ),
    (
        "modules/tabs/titlebars.kmodule.ron",
        include_str!("assets/modules/tabs/titlebars.kmodule.ron"),
    ),
    (
        "modules/tabs/tokens-anatomy.kmodule.ron",
        include_str!("assets/modules/tabs/tokens-anatomy.kmodule.ron"),
    ),
    (
        "modules/tabs/tokens-notes.kmodule.ron",
        include_str!("assets/modules/tabs/tokens-notes.kmodule.ron"),
    ),
    (
        "modules/tabs/tokens.kmodule.ron",
        include_str!("assets/modules/tabs/tokens.kmodule.ron"),
    ),
    (
        "modules/tabs/skins.kmodule.ron",
        include_str!("assets/modules/tabs/skins.kmodule.ron"),
    ),
    (
        "modules/tabs/table.kmodule.ron",
        include_str!("assets/modules/tabs/table.kmodule.ron"),
    ),
    (
        "modules/tabs/table-long.kmodule.ron",
        include_str!("assets/modules/tabs/table-long.kmodule.ron"),
    ),
    (
        "modules/tabs/tree.kmodule.ron",
        include_str!("assets/modules/tabs/tree.kmodule.ron"),
    ),
    (
        "modules/tabs/typography.kmodule.ron",
        include_str!("assets/modules/tabs/typography.kmodule.ron"),
    ),
    (
        "modules/tabs/vis-spacer.kmodule.ron",
        include_str!("assets/modules/tabs/vis-spacer.kmodule.ron"),
    ),
    (
        "modules/tabs/vis.kmodule.ron",
        include_str!("assets/modules/tabs/vis.kmodule.ron"),
    ),
    (
        "modules/titlebar.kmodule.ron",
        include_str!("assets/modules/titlebar.kmodule.ron"),
    ),
];

/// The gallery's documents on disk, laid over the ones this build embeds.
pub(crate) type Resolver = OverlayResolver<FileResolver, MemResolver>;

/// Where the gallery's own documents live, so editing one and opening the
/// gallery again shows the edit.
pub(crate) fn package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/gallery/assets")
}

/// The gallery reads its pages from the folder it ships them in, over the
/// built-in library.
///
/// The folder is named at build time and is part of this checkout, so it
/// being unreadable is a broken checkout rather than a runtime condition.
pub(crate) fn resolver() -> Resolver {
    let mut embedded = builtin::resolver();
    for (path, text) in ASSETS {
        embedded.insert(path, text);
    }
    let files = FileResolver::new(package_root()).expect("the gallery ships its own documents");
    OverlayResolver::new(files, embedded)
}

/// The file the gallery's package puts behind `role`.
///
/// A page states which screen it is; the manifest states which file that
/// screen lives in. Keeping the mapping in the package is what lets a page be
/// renamed, or replaced by another file, without touching this example.
///
/// # Panics
/// Panics when the package answers for no such role.
pub(crate) fn document(role: &str) -> &'static str {
    pages()
        .get(role)
        .unwrap_or_else(|| panic!("the gallery package answers for no screen {role}"))
}

/// Every role the gallery's package declares, and the file behind each.
///
/// Each file is read once here and checked against the role the manifest put
/// it behind, so a manifest that names the wrong file is refused where it is
/// read rather than drawn as the wrong page.
///
/// # Panics
/// Panics when the shipped manifest is unreadable or disagrees with a
/// document, which is a broken checkout rather than a runtime condition.
pub(crate) fn pages() -> &'static BTreeMap<ScreenRole, String> {
    static PAGES: LazyLock<BTreeMap<ScreenRole, String>> = LazyLock::new(|| {
        let resolver = resolver();
        let package = load_package(&resolver, "package.kpackage.ron")
            .unwrap_or_else(|error| panic!("the gallery ships a package manifest: {error}"));
        package
            .screens
            .keys()
            .map(|role| {
                let file = package.screen(&resolver, role).unwrap_or_else(|error| {
                    panic!("the gallery package must answer for {role}: {error}")
                });
                (role.clone(), file)
            })
            .collect()
    });

    &PAGES
}
