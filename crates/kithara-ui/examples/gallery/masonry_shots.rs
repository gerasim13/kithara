//! Headless Masonry capture: the same gallery documents, drawn by the Masonry
//! host into a Vello scene, rasterised off-screen and written as PNG.
//!
//! Enabled by `KITHARA_GALLERY_CAPTURE_MASONRY=<dir>`; it runs instead of the
//! iced window so the two hosts can be compared page by page.

use std::path::{Path, PathBuf};

use kithara_platform::time::Duration;
use kithara_ui::{
    app::{Config, Ui},
    builtin,
    capture::{Geometry, Offscreen, Stage, read_geometry, shoot_set},
};
use num_traits::cast::AsPrimitive;

use crate::{
    capture::{self, Shot},
    custom,
    fixture::{Consts, resolver},
    host::Gallery,
    mock,
};

/// Walks every gallery page through the Masonry host. Returns `false` when the
/// environment variable is absent, so the caller falls through to the window.
pub(super) fn run() -> bool {
    let Some(dir) = std::env::var_os("KITHARA_GALLERY_CAPTURE_MASONRY").map(PathBuf::from) else {
        return false;
    };
    match capture_set(&dir) {
        Ok(count) => println!("{count} page(s) written to {}", dir.display()),
        Err(error) => eprintln!("masonry capture failed: {error}"),
    }
    true
}

fn capture_set(dir: &Path) -> Result<usize, String> {
    let film = capture::requested()?.unwrap_or_else(capture::stills);
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
    let mut stage = Retained::new(config, frame(dir))?;
    shoot_set(&mut stage, &film, dir).map(|written| written.len())
}

/// The gallery drawn by the Masonry host into a Vello scene, one mounted
/// document at a time.
struct Retained<'config> {
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
}

impl<'config> Retained<'config> {
    fn new(config: Config<'config>, frame: Geometry) -> Result<Self, String> {
        Ok(Self {
            config,
            frame,
            off: Offscreen::new(frame.width, frame.height)?,
            page: None,
            pixels: Vec::new(),
        })
    }
}

impl Stage for Retained<'_> {
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
            page.frame(Duration::from_millis(Consts::STRESS_TICK_MS));
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

/// The geometry to photograph at: whatever an iced capture already sitting in
/// this directory used, so the two sets can be compared pixel for pixel.
/// Falls back to the gallery's own logical size at 1x.
fn frame(dir: &Path) -> Geometry {
    read_geometry(dir).unwrap_or_else(|| Geometry {
        height: AsPrimitive::<u32>::as_(Consts::HEIGHT),
        scale: 1.0,
        width: AsPrimitive::<u32>::as_(Consts::WIDTH),
    })
}
