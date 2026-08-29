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
    fs::create_dir_all,
    path::{Path, PathBuf},
};

use iced::window;
use kithara_ui::capture::{Geometry, Photographer, read_geometry, write_geometry, write_png};
use num_traits::cast::AsPrimitive;

use crate::{
    app::{theme, view},
    capture::Film,
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
    match capture(&dir) {
        Ok(count) => println!("{count} page(s) written to {}", dir.display()),
        Err(error) => eprintln!("offscreen capture failed: {error}"),
    }
    true
}

fn capture(dir: &PathBuf) -> Result<usize, String> {
    create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let frame = frame(dir);
    write_geometry(dir, frame)?;
    let mut gallery = crate::app::Gallery::mounted();
    let skin = gallery.skin();
    let theme = theme(skin);
    let mut photographer = Photographer::new()?;
    let film = Film::requested()?.unwrap_or_else(Film::stills);
    let mut written = 0;
    for &shot in &film.pages {
        gallery.select(shot);
        for photo in 0..film.photos {
            // Time passes between two photographs, never before the first: a
            // film opens where the page opens.
            if photo > 0 {
                for _ in 0..film.steps {
                    gallery.tick();
                }
            }
            let rgba = photographer.shoot(view(&gallery, window::Id::unique()), &theme, frame)?;
            write_png(
                &dir.join(film.file(shot, photo)),
                &rgba,
                frame.width,
                frame.height,
            )?;
            written += 1;
        }
    }
    Ok(written)
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
