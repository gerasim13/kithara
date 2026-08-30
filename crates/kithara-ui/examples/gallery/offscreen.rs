//! Photographs the gallery through iced with no window and no display.
//!
//! Asked for by `--shoot <dir>`. It writes the same pages, the same `frame.txt`
//! and the same images as the window capture, so any two of the three sets —
//! window, offscreen, masonry — compare on equal terms.
//!
//! Parity with the masonry capture is the point: that one has always been
//! headless, and while this one needed a display the two could only be compared
//! on a developer's machine. The window capture stays, and its job becomes
//! checking that this path draws what a window draws.

use std::path::Path;

use iced::{Theme, window};
use kithara_ui::capture::{Geometry, Photographer, Stage, shoot_set};

use crate::{
    app::{Gallery, theme, view},
    capture::Shot,
    cli::Args,
};

/// Photographs the film this run asked for, and says how many pictures landed.
pub(super) fn run(args: &Args, dir: &Path) -> Result<usize, String> {
    let film = args.film()?;
    let mut stage = Iced::new(args, args.frame(dir))?;
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
    fn new(args: &Args, frame: Geometry) -> Result<Self, String> {
        let mut gallery = Gallery::mounted();
        gallery.step = args.step();
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
