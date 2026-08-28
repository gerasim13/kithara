use std::{path::Path, rc::Rc};

use kithara_ui::{
    builtin,
    error::UiDocError,
    ids::{ScreenRole, SourceUri},
    package::{PackageDoc, load_package},
    render::Skin,
    skin::load_skin,
    source::{FileResolver, Limits, MemResolver, OverlayResolver, SourceResolver},
    text::{TextDoc, parse_text},
};

use super::cache::DeckLayout;

const DOCS: &[(&str, &str)] = &[
    (
        Package::MANIFEST,
        include_str!("../../../assets/ui/package.kpackage.ron"),
    ),
    (
        "app-en.ktext.ron",
        include_str!("../../../assets/ui/app-en.ktext.ron"),
    ),
    (
        "app.klayout.ron",
        include_str!("../../../assets/ui/app.klayout.ron"),
    ),
    (
        "app-single.klayout.ron",
        include_str!("../../../assets/ui/app-single.klayout.ron"),
    ),
    (
        "modules/app-bar.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-bar.kmodule.ron"),
    ),
    (
        "modules/app-bar-micro.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-bar-micro.kmodule.ron"),
    ),
    (
        "modules/app-menu.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu.kmodule.ron"),
    ),
    (
        "modules/app-menu/window-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu/window-row.kmodule.ron"),
    ),
    (
        "modules/app-menu/module-cell.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-menu/module-cell.kmodule.ron"),
    ),
    (
        "modules/app-deck.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-deck.kmodule.ron"),
    ),
    (
        "modules/app-overview.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview.kmodule.ron"),
    ),
    (
        "modules/app-overview-single.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview-single.kmodule.ron"),
    ),
    (
        "modules/app-overview-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-overview-row.kmodule.ron"),
    ),
    (
        "modules/app-mixer.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-mixer.kmodule.ron"),
    ),
    (
        "modules/app-mixer-single.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-mixer-single.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-mode-row.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-mode-row.kmodule.ron"),
    ),
    (
        "modules/app-strip.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-3-band.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-3-band.kmodule.ron"),
    ),
    (
        "modules/app-strip/eq-4-band.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-strip/eq-4-band.kmodule.ron"),
    ),
    (
        "modules/app-library.kmodule.ron",
        include_str!("../../../assets/ui/modules/app-library.kmodule.ron"),
    ),
];

/// What this application asks a package for, one role per deck arrangement.
///
/// The package decides which file stands behind each, so a package may rename
/// its screens without this having to know.
fn role(layout: DeckLayout) -> ScreenRole {
    ScreenRole(
        match layout {
            DeckLayout::Single => "deck-single",
            DeckLayout::Dual => "deck-dual",
        }
        .to_owned(),
    )
}

/// One loaded UI package: the documents it is read through, the screens it
/// answered for, and the skin and catalog it dresses them in.
///
/// Everything a host needs to draw this application comes from here, so the
/// window and the pages it shows are never read through two different packages.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Package {
    resolver: Box<dyn SourceResolver>,
    screens: Screens,
    /// The skin this package dresses its pages in, resolved once.
    #[field(get, vis = "pub(in crate::gui)")]
    skin: Skin,
    /// The captions every `@key` in this package's documents resolves against.
    #[field(get, vis = "pub(in crate::gui)")]
    text: TextDoc,
}

impl Package {
    /// The manifest naming everything this application asks a package for.
    const MANIFEST: &'static str = "package.kpackage.ron";
    /// The paths this application cannot be itself without.
    ///
    /// A package lays its screens out as it likes, and almost everything here
    /// is free: which modules stand where, what they are called, what they are
    /// dressed in. These two are not. `deck-a/play` is the only path that
    /// starts and stops playback, and `deck-a/wave` the only one that moves the
    /// position within a track; a screen offering neither draws a player that
    /// cannot play, and nothing about drawing it would say so.
    pub(crate) const REQUIRED: &'static [&'static str] = &["deck-a/play", "deck-a/wave"];

    /// Reads the package laid out at `root` over the documents this build
    /// carries, or only those documents when `root` names nothing.
    ///
    /// A path that does not exist means no package was laid out. Anything else
    /// that stops the package being read - a permission, a broken manifest -
    /// is an error rather than a quiet return to the built-in documents.
    pub(crate) fn load(root: Option<&Path>) -> Result<Rc<Self>, UiDocError> {
        match root.filter(|root| root.exists()) {
            Some(root) => {
                let files = FileResolver::new(root).map_err(|error| UiDocError::Unreadable {
                    origin: SourceUri(root.display().to_string()),
                    rel: String::new(),
                    source: error,
                })?;
                Self::read(Box::new(OverlayResolver::new(files, embedded())))
            }
            None => Self::read(Box::new(embedded())),
        }
    }

    fn read(resolver: Box<dyn SourceResolver>) -> Result<Rc<Self>, UiDocError> {
        let manifest = load_package(resolver.as_ref(), Self::MANIFEST)?;
        let screens = Screens::resolve(&manifest, resolver.as_ref())?;
        let text = catalog(resolver.as_ref(), &manifest)?;
        let skin = dress(resolver.as_ref(), &manifest, &text)?;
        Ok(Rc::new(Self {
            resolver,
            screens,
            skin,
            text,
        }))
    }

    pub(in crate::gui) fn document(&self, layout: DeckLayout) -> &str {
        self.screens.document(layout)
    }

    pub(in crate::gui) fn resolver(&self) -> &dyn SourceResolver {
        self.resolver.as_ref()
    }
}

/// The screen this application asks a package for, one per deck arrangement.
///
/// Both are resolved once, when the package is read, so a host that has to
/// name the document it draws can hand back a name the package already
/// answered for.
struct Screens {
    dual: String,
    single: String,
}

impl Screens {
    fn resolve(manifest: &PackageDoc, resolver: &dyn SourceResolver) -> Result<Self, UiDocError> {
        Ok(Self {
            dual: manifest.screen(resolver, &role(DeckLayout::Dual))?,
            single: manifest.screen(resolver, &role(DeckLayout::Single))?,
        })
    }

    fn document(&self, layout: DeckLayout) -> &str {
        match layout {
            DeckLayout::Single => &self.single,
            DeckLayout::Dual => &self.dual,
        }
    }
}

/// The caption catalog the documents resolve `@key` against: the built-in one
/// with the entries the package names laid over it.
///
/// A package that names no catalog of its own draws with the built-in words.
fn catalog(resolver: &dyn SourceResolver, manifest: &PackageDoc) -> Result<TextDoc, UiDocError> {
    let Some(rel) = manifest.text.as_deref() else {
        return Ok(builtin::text_doc().clone());
    };
    let loaded = resolver.load(None, rel)?;
    let extra = parse_text(&loaded.text, &loaded.uri)?;
    builtin::text_doc().merge(&extra, &loaded.uri)
}

/// The skin the package names, resolved against that same package: a package
/// carrying its own skin document dresses every page it ships.
///
/// A package that names no skin of its own wears the built-in one.
fn dress(
    resolver: &dyn SourceResolver,
    manifest: &PackageDoc,
    text: &TextDoc,
) -> Result<Skin, UiDocError> {
    let Some(rel) = manifest.skin.as_deref() else {
        return Ok(builtin::skin().clone());
    };
    let document = load_skin(resolver, rel, &Limits::default())?;
    Skin::resolve(document, text, &SourceUri(rel.to_owned()), resolver)
}

/// Where this application's own documents are read from when nothing is laid
/// out on disk: the built-in library with this build's own documents over it.
pub(crate) fn embedded() -> MemResolver {
    let mut resolver = builtin::resolver();
    for (path, text) in DOCS {
        resolver.insert(path, text);
    }
    resolver
}
