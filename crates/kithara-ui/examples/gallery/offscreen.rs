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
    gallery: Gallery,
    frame: Geometry,
    photographer: Photographer,
    theme: Theme,
    /// The pixels of the page last photographed. Held here because the walk
    /// borrows them rather than owning storage it cannot size.
    pixels: Vec<u8>,
}

impl Iced {
    fn new(args: &Args, frame: Geometry) -> Result<Self, String> {
        let mut gallery = Gallery::mounted();
        gallery.step = args.step();
        let theme = theme(gallery.skin());
        Ok(Self {
            frame,
            gallery,
            theme,
            photographer: Photographer::new()?,
            pixels: Vec::new(),
        })
    }
}

impl Stage for Iced {
    type Page = Shot;

    fn geometry(&self) -> Geometry {
        self.frame
    }

    fn shoot(&mut self) -> Result<&[u8], String> {
        self.pixels = self.photographer.shoot(
            view(&self.gallery, window::Id::unique()),
            &self.theme,
            self.frame,
        )?;
        Ok(&self.pixels)
    }

    fn tick(&mut self) {
        self.gallery.tick();
    }

    fn turn(&mut self, page: &Shot) -> Result<(), String> {
        self.gallery.select(*page);
        Ok(())
    }
}
