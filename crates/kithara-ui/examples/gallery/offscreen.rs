//! Photographs the gallery through iced with no window and no display.
//!
//! Enabled by `KITHARA_GALLERY_CAPTURE_OFFSCREEN=<dir>`. It writes the same
//! pages, the same `frame.txt` and the same images as the window capture, so any
//! two of the three sets — window, offscreen, masonry — compare on equal terms.
//!
//! Parity with the masonry capture is the point: that one has always been
//! headless, and while this one needed a display the two could only be compared
//! on a developer's machine. The window capture stays, and its job becomes
//! checking that this path draws what a window draws.

use std::{
    env,
    path::{Path, PathBuf},
};

use iced::{Theme, window};
use kithara_ui::capture::{Geometry, Photographer, Stage, read_geometry, shoot_set};
use num_traits::cast::AsPrimitive;

use crate::{
    app::{Gallery, theme, view},
    capture::{self, Shot},
};

/// The scale the offscreen set is photographed at when the directory does not
/// already say. One, so a page's pixels are its points and a difference between
/// two sets is a difference in what was drawn rather than in how it was sampled.
const SCALE: f32 = 1.0;

/// Runs the capture when asked. Returns `false` when the environment variable
/// is absent, so the caller falls through to the window.
pub(super) fn run() -> bool {
    let Some(dir) = env::var_os("KITHARA_GALLERY_CAPTURE_OFFSCREEN").map(PathBuf::from) else {
        return false;
    };
    match capture_set(&dir) {
        Ok(count) => println!("{count} page(s) written to {}", dir.display()),
        Err(error) => eprintln!("offscreen capture failed: {error}"),
    }
    true
}

fn capture_set(dir: &Path) -> Result<usize, String> {
    let film = capture::requested()?.unwrap_or_else(capture::stills);
    let mut stage = Iced::new(frame(dir))?;
    shoot_set(&mut stage, &film, dir).map(|written| written.len())
}

/// The gallery drawn by iced into a texture, one page at a time.
struct Iced {
    frame: Geometry,
    gallery: Gallery,
    photographer: Photographer,
    /// The pixels of the page last photographed. Held here because the walk
    /// borrows them rather than owning storage it cannot size.
    pixels: Vec<u8>,
    theme: Theme,
}

impl Iced {
    fn new(frame: Geometry) -> Result<Self, String> {
        let gallery = Gallery::mounted();
        let theme = theme(gallery.skin());
        Ok(Self {
            frame,
            gallery,
            photographer: Photographer::new()?,
            pixels: Vec::new(),
            theme,
        })
    }
}

impl Stage for Iced {
    type Page = Shot;

    fn geometry(&self) -> Geometry {
        self.frame
    }

    fn turn(&mut self, page: &Shot) -> Result<(), String> {
        self.gallery.select(*page);
        Ok(())
    }

    fn tick(&mut self) {
        self.gallery.tick();
    }

    fn shoot(&mut self) -> Result<&[u8], String> {
        self.pixels = self.photographer.shoot(
            view(&self.gallery, window::Id::unique()),
            &self.theme,
            self.frame,
        )?;
        Ok(&self.pixels)
    }
}

/// The geometry to photograph at: whatever a capture already sitting in this
/// directory used, so a set taken on a display — where the scale is the
/// screen's, not ours — can be answered on its own terms. Falls back to the
/// gallery's own logical size at 1x, which is what the window opens at.
fn frame(dir: &Path) -> Geometry {
    read_geometry(dir).unwrap_or_else(|| {
        let size = crate::app::window_size();
        Geometry {
            height: AsPrimitive::<u32>::as_(size.height * SCALE),
            scale: SCALE.into(),
            width: AsPrimitive::<u32>::as_(size.width * SCALE),
        }
    })
}
