//! The studio photographed through iced with no window.

use std::rc::Rc;

use iced::Theme;
use kithara::ui::{
    builtin,
    capture::{Geometry, Photographer, Stage},
    compile::CompiledUi,
    render::{Clock, tree},
};

use super::{
    fixture::Fixture,
    page::{Page, PoolSample, pooled},
};
use crate::{
    gui::{
        theme::kithara_theme,
        ui::{self, package::Package},
    },
    theme::Palette,
};

/// The studio drawn by iced into a texture, one layout at a time.
pub(super) struct Immediate {
    geometry: Geometry,
    open: Option<(Page, CompiledUi, Fixture)>,
    package: Rc<Package>,
    photographer: Photographer,
    pixels: Vec<u8>,
    pub(super) pools: String,
    theme: Theme,
}

impl Immediate {
    pub(super) fn new(geometry: Geometry) -> Result<Self, String> {
        Ok(Self {
            geometry,
            open: None,
            package: Package::load(None).map_err(|error| format!("package: {error}"))?,
            photographer: Photographer::new()?,
            pixels: Vec::new(),
            pools: String::new(),
            theme: kithara_theme(&Palette::default().into()),
        })
    }
}

impl Stage for Immediate {
    type Page = Page;

    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn turn(&mut self, page: &Page) -> Result<(), String> {
        let compiled = ui::compile_ui(page.0)
            .map_err(|error| format!("compile {}: {error}", self.package.document(page.0)))?;
        let reads = Fixture::new(page.0, Rc::clone(&self.package));
        self.open = Some((*page, compiled, reads));
        Ok(())
    }

    fn tick(&mut self) {}

    fn shoot(&mut self) -> Result<&[u8], String> {
        let (page, compiled, reads) = self
            .open
            .as_ref()
            .ok_or_else(|| "no layout is open: turn to one before photographing".to_owned())?;
        let skin = builtin::skin();
        let draw = || {
            tree::render(
                &compiled.root,
                compiled,
                reads,
                skin,
                Clock::default(),
                None,
            )
        };
        self.pixels = self
            .photographer
            .shoot(draw(), &self.theme, self.geometry)?;
        // The second page is drawn for the pools alone: a frame that allocates
        // again once the pools are warm is the defect this watches for.
        let first = compiled.draw_pool_stats();
        drop(
            self.photographer
                .shoot(draw(), &self.theme, self.geometry)?,
        );
        let sample = PoolSample {
            first,
            second: compiled.draw_pool_stats(),
        };
        pooled(&mut self.pools, *page, &sample)?;
        Ok(&self.pixels)
    }
}
