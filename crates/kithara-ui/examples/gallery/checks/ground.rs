//! Whether a page covers the window it is given.
//!
//! The window keeps no ground of its own: it opens transparent and the host
//! clears it to nothing, so every pixel a page shows is a pixel some document
//! painted. Whatever the documents leave bare is not the skin's page colour —
//! it is the desktop, showing through the application.
//!
//! Read on the retained host, whose rasteriser this toolkit already drives
//! headless, and cleared to nothing rather than to the skin's page colour: the
//! capture sets are cleared to that colour, which is exactly the colour a hole
//! would have been painted, so a set taken there cannot see one.
//!
//! The claim holds for this gallery's skin, whose window corners are square. A
//! skin that rounds them buys those corners from the desktop on purpose, and
//! would have to be asked a shape-aware question instead of this one.

use kithara_test_utils::kithara;
use kithara_ui::{
    app::{Config, Ui},
    builtin,
};
use masonry::vello::peniko::Color;
use num_traits::cast::AsPrimitive;

use crate::{
    Consts, capture::Shot, custom, host::Gallery, masonry_shots::Offscreen, mock, resolver,
};

#[kithara::test]
fn every_page_paints_over_the_whole_window() {
    let width: u32 = AsPrimitive::<u32>::as_(Consts::WIDTH);
    let height: u32 = AsPrimitive::<u32>::as_(Consts::HEIGHT);
    let endpoints = mock::registry();
    let resolver = resolver();
    let kinds = custom::kinds();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .kinds(&kinds)
        .build();
    let mut off = Offscreen::new(width, height)
        .unwrap_or_else(|error| panic!("the pages must rasterise: {error}"));

    let bare: Vec<String> = Shot::all()
        .into_iter()
        .filter_map(|page| {
            let mut ui = Ui::new(Gallery::at(page), config, (width, height), 1.0)
                .unwrap_or_else(|error| panic!("page {} must mount: {error}", page.name()));
            let frame = ui
                .render()
                .unwrap_or_else(|error| panic!("page {} must draw: {error}", page.name()));
            let rgba = off
                .rasterise(&frame, 1.0, Color::TRANSPARENT)
                .unwrap_or_else(|error| panic!("page {} must rasterise: {error}", page.name()));
            let stride: usize = AsPrimitive::<usize>::as_(width);
            let pixels = rgba.len() / 4;
            let bare: Vec<(usize, u8)> = rgba
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .enumerate()
                .filter(|(_, alpha)| *alpha != u8::MAX)
                .collect();
            let (first, thinnest) = bare.first().copied()?;
            Some(format!(
                "{}: {} of {pixels} pixel(s), first at {}x{} showing through at alpha {thinnest}",
                page.name(),
                bare.len(),
                first % stride,
                first / stride,
            ))
        })
        .collect();

    assert!(
        bare.is_empty(),
        "these pages leave part of the window unpainted, and the window has no ground of its own, \
         so what shows there is the desktop behind the application: {bare:#?}"
    );
}
