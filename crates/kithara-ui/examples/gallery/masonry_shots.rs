//! Headless Masonry capture: the same gallery documents, drawn by the Masonry
//! host into a Vello scene, rasterised off-screen and written as PNG.
//!
//! Enabled by `KITHARA_GALLERY_CAPTURE_MASONRY=<dir>`; it runs instead of the
//! iced window so the two hosts can be compared page by page.

use std::{
    env,
    fs::create_dir_all,
    path::{Path, PathBuf},
};

use kithara_platform::time::Duration;
use kithara_ui::{
    app::{Config, Ui},
    builtin,
    capture::{Geometry, Offscreen, read_geometry, write_geometry, write_png},
};
use num_traits::cast::AsPrimitive;

use crate::{
    capture::Film,
    custom,
    fixture::{Consts, resolver},
    host::Gallery,
    mock,
};

/// Walks every gallery page through the Masonry host. Returns `false` when the
/// environment variable is absent, so the caller falls through to the window.
pub(super) fn run() -> bool {
    let Some(dir) = env::var_os("KITHARA_GALLERY_CAPTURE_MASONRY").map(PathBuf::from) else {
        return false;
    };
    match capture(&dir) {
        Ok(count) => println!("{count} page(s) written to {}", dir.display()),
        Err(error) => eprintln!("masonry capture failed: {error}"),
    }
    true
}

fn capture(dir: &PathBuf) -> Result<usize, String> {
    create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let frame = frame(dir);
    let mut off = Offscreen::new(frame.width, frame.height)?;
    let resolver = resolver();
    let endpoints = mock::registry();
    let kinds = custom::kinds();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .kinds(&kinds)
        .build();
    write_geometry(dir, frame)?;
    let film = Film::requested()?.unwrap_or_else(Film::stills);
    let mut written = 0;
    // One buffer for the whole set: every page is the same geometry, so the
    // pixels of the last page are the storage of the next.
    let mut rgba = Vec::new();

    for &shot in &film.pages {
        let mut ui = Ui::new(
            Gallery::at(shot),
            config,
            (frame.width, frame.height),
            frame.scale,
        )
        .map_err(|error| format!("mount {}: {error}", shot.name()))?;
        for photo in 0..film.photos {
            // Time passes between two photographs, never before the first: a
            // film opens where the page opens.
            if photo > 0 {
                for _ in 0..film.steps {
                    ui.frame(Duration::from_millis(Consts::STRESS_TICK_MS));
                }
            }
            let ui_frame = ui
                .render()
                .map_err(|error| format!("draw {}: {error}", shot.name()))?;
            off.rasterise(&ui_frame, frame.scale, ui.background().into(), &mut rgba)?;
            let path = dir.join(film.file(shot, photo));
            write_png(&path, &rgba, frame.width, frame.height)?;
            println!("captured {}", path.display());
            written += 1;
        }
    }
    Ok(written)
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
