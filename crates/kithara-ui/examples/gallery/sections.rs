//! Which pages the gallery offers, taken from the package it ships.
//!
//! A page is added by putting its document in the folder, naming it in the
//! manifest and giving it a nav item. Nothing here lists the pages, so the
//! list cannot disagree with the package that answers for them.

use std::sync::LazyLock;

use crate::fixture;

/// What a page's nav item, its document and the reading behind both call it.
///
/// The manifest answers for the screen `gallery-<slug>`, a press arrives from
/// `gallery/<slug>/item`, and a photograph of the page is named `<slug>`.
pub(super) type Page = &'static str;

/// The page the gallery opens on, which is the one its nav lists first.
pub(super) const FIRST: Page = "atoms";
/// The page whose demos the modules list offers.
pub(super) const MODULES: Page = "modules";

/// What the gallery's package calls the screens this module reads.
struct Consts;

impl Consts {
    /// What the modules page calls the demo it opens on. That demo's document
    /// is the modules page itself, so the manifest names it once and this is
    /// the only demo whose slug the manifest does not carry.
    const FIRST_MODULE: Page = "deck";
    /// The prefix every screen the gallery's package declares stands behind.
    const ROLE: &'static str = "gallery-";
}

/// Every page the nav lists, in the order the package declares them.
pub(super) fn pages() -> &'static [Page] {
    &declared().0
}

/// The demos the modules page offers, the one it opens on first.
pub(super) fn modules() -> &'static [Page] {
    &declared().1
}

/// Where `page` stands among the pages, which is the order they are compiled
/// in.
///
/// # Panics
/// Panics when asked for a page the package does not declare.
pub(super) fn index(page: Page) -> usize {
    position(pages(), page)
}

/// Where `module` stands among the modules page's demos.
///
/// # Panics
/// Panics when asked for a demo the modules page does not offer.
pub(super) fn module_index(module: Page) -> usize {
    position(modules(), module)
}

/// The file the package puts behind a nav page.
pub(super) fn entry(page: Page) -> &'static str {
    fixture::document(&format!("{}{page}", Consts::ROLE))
}

/// The file the package puts behind one demo of the modules page.
pub(super) fn module_entry(module: Page) -> &'static str {
    if module == Consts::FIRST_MODULE {
        return entry(MODULES);
    }
    fixture::document(&format!("{}{MODULES}-{module}", Consts::ROLE))
}

/// The page a press on a nav item turns to, or nothing when the press came
/// from somewhere else.
pub(super) fn pressed(path: &str) -> Option<Page> {
    named(path.strip_prefix("gallery/")?.strip_suffix("/item")?)
}

/// The page named `slug`, or nothing when the package declares no such page.
pub(super) fn named(slug: &str) -> Option<Page> {
    pages().iter().copied().find(|page| *page == slug)
}

/// The demo of the modules page named `slug`, or nothing when it offers none.
pub(super) fn module_named(slug: &str) -> Option<Page> {
    modules().iter().copied().find(|module| *module == slug)
}

/// The nav pages and the modules page's demos, split out of the roles the
/// package declares.
///
/// # Panics
/// Panics when the manifest declares a screen the gallery is not the package
/// for, which is a broken checkout rather than a runtime condition.
fn declared() -> &'static (Vec<Page>, Vec<Page>) {
    static DECLARED: LazyLock<(Vec<Page>, Vec<Page>)> = LazyLock::new(|| {
        let demo = format!("{MODULES}-");
        let mut pages = Vec::new();
        let mut modules = vec![Consts::FIRST_MODULE];
        for role in fixture::pages().keys() {
            let slug = role
                .0
                .strip_prefix(Consts::ROLE)
                .unwrap_or_else(|| panic!("the gallery package declares a screen {role}"));
            match slug.strip_prefix(&demo) {
                Some(module) => modules.push(module),
                None => pages.push(slug),
            }
        }
        (pages, modules)
    });

    &DECLARED
}

fn position(among: &[Page], page: Page) -> usize {
    among
        .iter()
        .position(|named| *named == page)
        .unwrap_or_else(|| panic!("the gallery offers no page {page}"))
}
