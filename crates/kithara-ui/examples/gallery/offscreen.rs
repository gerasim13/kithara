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

use std::{borrow::Cow, env, fs::create_dir_all, path::PathBuf};

use iced::{
    Color, Rectangle, Size,
    advanced::{
        graphics::{Viewport, text::font_system},
        layout::{Layout, Limits},
        mouse::Cursor,
        renderer::Style,
        widget::Tree,
    },
    window,
};
use iced_renderer::fallback::Renderer as FallbackRenderer;
use iced_tiny_skia::Renderer as TinySkiaRenderer;
use kithara_ui::render::fonts::{FONT_BYTES, SANS};
use num_traits::cast::AsPrimitive;

use super::{
    capture::{Frame, Shot, write_frame, write_png},
    theme, view,
};

/// The scale the offscreen set is photographed at. One, so a page's pixels are
/// its points and a difference between two sets is a difference in what was
/// drawn rather than in how it was sampled.
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
    let frame = frame();
    write_frame(dir, frame)?;
    let mut gallery = super::Gallery::mounted();
    let skin = gallery.skin;
    let theme = theme(skin);
    let mut written = 0;
    for shot in Shot::all() {
        gallery.select(shot);
        let rgba = page(&gallery, &theme, frame)?;
        write_png(
            &dir.join(format!("{}.png", shot.name())),
            &rgba,
            frame.width,
            frame.height,
        )?;
        written += 1;
    }
    Ok(written)
}

/// One page, laid out and rasterised at the frame's geometry.
fn page(gallery: &super::Gallery, theme: &iced::Theme, frame: Frame) -> Result<Vec<u8>, String> {
    let mut element = view(gallery, window::Id::unique());
    let mut renderer = renderer();
    let logical = Size::new(
        AsPrimitive::<f32>::as_(frame.width) / SCALE,
        AsPrimitive::<f32>::as_(frame.height) / SCALE,
    );
    let mut tree = Tree::new(element.as_widget());
    let node =
        element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &Limits::new(Size::ZERO, logical));
    let bounds = Rectangle::with_size(logical);
    element.as_widget().draw(
        &tree,
        &mut renderer,
        theme,
        &Style::default(),
        Layout::new(&node),
        Cursor::Unavailable,
        &bounds,
    );
    let FallbackRenderer::Secondary(renderer) = &mut renderer else {
        return Err("the offscreen capture must rasterise through tiny-skia".to_owned());
    };
    let viewport = Viewport::with_physical_size(
        Size::new(frame.width, frame.height),
        AsPrimitive::<f32>::as_(frame.scale),
    );
    Ok(iced_tiny_skia::window::compositor::screenshot(
        renderer,
        &viewport,
        Color::BLACK,
    ))
}

/// A renderer with the gallery's own faces registered, drawing into memory
/// rather than into a surface.
fn renderer() -> iced::Renderer {
    let mut fonts = font_system()
        .write()
        .unwrap_or_else(|error| panic!("iced font system lock must be available: {error}"));
    for bytes in FONT_BYTES {
        fonts.load_font(Cow::Borrowed(bytes));
    }
    drop(fonts);

    FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, iced::Pixels(14.0)))
}

/// The geometry every set is photographed at. The window opens at the same
/// size, so the three sets line up without anyone having to say so twice.
fn frame() -> Frame {
    let size = super::window_size();
    Frame {
        height: AsPrimitive::<u32>::as_(size.height * SCALE),
        scale: SCALE.into(),
        width: AsPrimitive::<u32>::as_(size.width * SCALE),
    }
}
