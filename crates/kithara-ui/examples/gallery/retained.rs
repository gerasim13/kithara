//! The gallery through the retained host: shown in a window, or drawn into a
//! Vello scene, rasterised off-screen and written as PNG.
//!
//! `--host retained` shows it; adding `--shoot <dir>` photographs it instead,
//! which is how the two hosts are compared page by page.

use std::path::Path;

use kithara_platform::time::Duration;
use kithara_ui::{
    app::{Config, Ui},
    builtin,
    capture::{Geometry, Offscreen, Stage, shoot_set},
};
use num_traits::cast::AsPrimitive;

use crate::{
    capture::Shot,
    cli::{Args, Extent},
    custom,
    fixture::resolver,
    host::Gallery,
    mock,
};

/// A size in whole pixels, which is what this host opens a window at.
fn pixels(extent: Extent) -> (u32, u32) {
    (
        AsPrimitive::<u32>::as_(extent.width),
        AsPrimitive::<u32>::as_(extent.height),
    )
}

/// Shows the gallery in a window of this host's.
pub(super) fn show(args: &Args) -> Result<(), String> {
    let endpoints = mock::registry();
    let resolver = resolver();
    let kinds = custom::kinds();
    // The document carries its own title bar and window buttons, so the system
    // frame stays off, exactly as it does under the other host.
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .kinds(&kinds)
        .decorations(false)
        .min_size(pixels(args.min_size))
        .title("Kithara UI Gallery")
        .build();
    kithara_ui::app::run(Gallery::default(), config, pixels(args.size))
        .map_err(|error| format!("gallery did not run: {error}"))
}

/// Photographs the film this run asked for, and says how many pictures landed.
pub(super) fn shoot(args: &Args, dir: &Path) -> Result<usize, String> {
    let film = args.film()?;
    // The registries outlive the stage that borrows them through the config.
    let resolver = resolver();
    let endpoints = mock::registry();
    let kinds = custom::kinds();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .kinds(&kinds)
        .build();
    let mut stage = Masonry::new(config, args.frame(dir), args.step())?;
    shoot_set(&mut stage, &film, dir).map(|written| written.len())
}

/// The gallery drawn by the Masonry host into a Vello scene, one mounted
/// document at a time.
struct Masonry<'config> {
    config: Config<'config>,
    frame: Geometry,
    off: Offscreen,
    /// The page currently open. A stage holds no document until it is turned to
    /// one, and turning to a page mounts it: a gallery page is a document of its
    /// own, not a view of one shared tree.
    page: Option<Ui<'config, Gallery>>,
    /// The pixels read back from the last page photographed. Held here because
    /// the walk borrows them rather than owning storage it cannot size.
    pixels: Vec<u8>,
    /// How far this host's clock moves in one frame.
    step: Duration,
}

impl<'config> Masonry<'config> {
    fn new(config: Config<'config>, frame: Geometry, step: Duration) -> Result<Self, String> {
        Ok(Self {
            config,
            frame,
            off: Offscreen::new(frame.width, frame.height)?,
            page: None,
            pixels: Vec::new(),
            step,
        })
    }
}

impl Stage for Masonry<'_> {
    type Page = Shot;

    fn geometry(&self) -> Geometry {
        self.frame
    }

    fn turn(&mut self, page: &Shot) -> Result<(), String> {
        self.page = Some(
            Ui::new(
                Gallery::at(*page),
                self.config,
                (self.frame.width, self.frame.height),
                self.frame.scale,
            )
            .map_err(|error| format!("mount {page}: {error}"))?,
        );
        Ok(())
    }

    fn tick(&mut self) {
        if let Some(page) = self.page.as_mut() {
            page.frame(self.step);
        }
    }

    fn shoot(&mut self) -> Result<&[u8], String> {
        let page = self
            .page
            .as_mut()
            .ok_or_else(|| "no page is open: turn to one before photographing".to_owned())?;
        let drawn = page.render().map_err(|error| format!("draw: {error}"))?;
        let background = page.background().into();
        self.off
            .rasterise(&drawn, self.frame.scale, background, &mut self.pixels)?;
        Ok(&self.pixels)
    }
}
