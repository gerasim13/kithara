//! The studio capture itself: two layouts through both hosts, written as two
//! sets the parity lane compares.

use std::{
    env,
    fs::write,
    path::{Path, PathBuf},
    rc::Rc,
};

use ::kithara::ui::{
    app::Config,
    capture::{Film, shoot_set},
};
use kithara_test_utils::kithara;

use super::{
    immediate::Immediate,
    page::{Page, studio},
    retained::Retained,
};
use crate::gui::ui::{cache::DeckLayout, endpoints::Registry, package::Package};

#[kithara::test]
fn studio_capture_writes_both_hosts() {
    let Some(dir) = env::var_os("KITHARA_STUDIO_CAPTURE").map(PathBuf::from) else {
        return;
    };
    capture(&dir).unwrap_or_else(|error| panic!("studio capture failed: {error}"));
}

fn capture(dir: &Path) -> Result<(), String> {
    let geometry = studio();
    let film = Film::stills(vec![Page(DeckLayout::Single), Page(DeckLayout::Dual)]);

    let mut immediate = Immediate::new(geometry)?;
    let iced_dir = dir.join("iced");
    shoot_set(&mut immediate, &film, &iced_dir)?;
    write(iced_dir.join("draw-pools.txt"), &immediate.pools)
        .map_err(|error| format!("write iced draw-pools.txt: {error}"))?;

    // The package and the registry the retained host reads through outlive the
    // stage that borrows them.
    let package = Package::load(None).map_err(|error| format!("package: {error}"))?;
    let endpoints = Registry::default();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(package.resolver())
        .text(package.text())
        .build();
    let mut retained = Retained::new(Rc::clone(&package), config, geometry)?;
    let masonry_dir = dir.join("masonry");
    shoot_set(&mut retained, &film, &masonry_dir)?;
    write(masonry_dir.join("draw-pools.txt"), &retained.pools)
        .map_err(|error| format!("write masonry draw-pools.txt: {error}"))
}
