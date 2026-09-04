use std::{
    fmt::Display,
    fs::create_dir_all,
    ops::Range,
    path::{Path, PathBuf},
};

use num_traits::cast::AsPrimitive;

use super::{Geometry, Stage, write_png};
use crate::draw::Rect;

/// A rectangle of one photograph, in that photograph's own pixels.
///
/// A caller builds one to say which part of a frame a picture is cut from, so
/// its fields stay open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub height: u32,
    pub width: u32,
    pub x: u32,
    pub y: u32,
}

impl Region {
    /// The pixels a rectangle laid out in points covers in a photograph taken
    /// at this scale.
    ///
    /// # Errors
    /// Refuses a rectangle that starts before the frame does, or that covers
    /// nothing: rounding either one into the frame would photograph somewhere
    /// else and report it as the control that was asked for.
    pub fn of(rect: Rect, scale: f64) -> Result<Self, String> {
        if rect.x < 0.0 || rect.y < 0.0 {
            return Err(format!(
                "a control at {},{} is laid out before the frame begins",
                rect.x, rect.y
            ));
        }
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return Err(format!(
                "a control of {}x{} points covers nothing",
                rect.w, rect.h
            ));
        }
        let scale: f32 = scale.as_();
        Ok(Self {
            height: (rect.h * scale).round().as_(),
            width: (rect.w * scale).round().as_(),
            x: (rect.x * scale).round().as_(),
            y: (rect.y * scale).round().as_(),
        })
    }

    /// Where each row of this region sits in the pixels of that frame.
    ///
    /// # Errors
    /// Refuses a region that covers no pixel or that leaves the frame, and a
    /// frame the pixels do not fill. A region is never clipped to fit: a
    /// picture smaller than the control that was asked for is not a picture of
    /// it, and a run that photographed one would report it as one that did.
    fn rows(
        self,
        frame: Geometry,
        pixels: &[u8],
    ) -> Result<impl Iterator<Item = Range<usize>>, String> {
        /// How many bytes one pixel of a photograph carries.
        const CHANNELS: usize = 4;

        if self.width == 0 || self.height == 0 {
            return Err(format!(
                "a region of {}x{} pixels photographs nothing",
                self.width, self.height
            ));
        }
        let corner = self
            .x
            .checked_add(self.width)
            .zip(self.y.checked_add(self.height));
        let Some((_, bottom)) =
            corner.filter(|&(right, bottom)| right <= frame.width && bottom <= frame.height)
        else {
            return Err(format!(
                "a region of {}x{} at {},{} leaves the {}x{} frame",
                self.width, self.height, self.x, self.y, frame.width, frame.height
            ));
        };
        let stride = AsPrimitive::<usize>::as_(frame.width) * CHANNELS;
        let filled = stride * AsPrimitive::<usize>::as_(frame.height);
        if pixels.len() != filled {
            return Err(format!(
                "a {}x{} frame is {filled} bytes, got {}",
                frame.width,
                frame.height,
                pixels.len()
            ));
        }
        let width = AsPrimitive::<usize>::as_(self.width) * CHANNELS;
        let start = AsPrimitive::<usize>::as_(self.x) * CHANNELS;
        let rows = AsPrimitive::<usize>::as_(self.y)..AsPrimitive::<usize>::as_(bottom);
        Ok(rows.map(move |row| {
            let from = row * stride + start;
            from..from + width
        }))
    }
}

/// What a stage answers when it knows where the controls it drew ended up.
///
/// Only a host that keeps its tree between frames can say: an immediate host
/// builds and forgets the tree inside one draw, and never named what it built.
/// It is a trait of its own rather than a method on [`Stage`] for exactly that
/// reason - a host with no answer should not compile against the question.
pub trait Locate {
    /// Where the control at this document path was laid out, in the logical
    /// points the host lays out in, or `None` when the open page draws no such
    /// control.
    fn locate(&self, path: &str) -> Option<Rect>;
}

/// Photographs one control of one page into a directory, answering with the
/// file written.
///
/// One picture rather than a set: a set is photographed on one geometry for
/// every page in it, and a control is laid out to a different rectangle on
/// every page that draws it, so a set of controls has no frame to record.
///
/// # Errors
/// Fails when the directory cannot be made, when the stage cannot open or draw
/// the page, when the page draws no control at that path, or when the region
/// the control was laid out in cannot be cut out of the photograph.
pub fn shoot_part<S>(
    stage: &mut S,
    page: &S::Page,
    path: &str,
    dir: &Path,
) -> Result<PathBuf, String>
where
    S: Stage + Locate,
    S::Page: Display,
{
    create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    stage.turn(page)?;
    let frame = stage.geometry();
    let rect = stage
        .locate(path)
        .ok_or_else(|| format!("{page} draws no control at {path}"))?;
    let region = Region::of(rect, frame.scale)?;
    let file = dir.join(part_file(page, path));
    let pixels = stage.shoot()?;
    let rows = region.rows(frame, pixels)?;
    write_png(
        &file,
        region.width,
        region.height,
        rows.map(|row| &pixels[row]),
    )?;
    Ok(file)
}

/// Where a photograph of one control lands: the page it was drawn on and the
/// control's own path, whose separators a file name cannot carry.
pub fn part_file<Page: Display>(page: &Page, path: &str) -> String {
    let control: String = path
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char
            } else {
                '-'
            }
        })
        .collect();
    format!("{page}-{control}.png")
}

#[cfg(test)]
mod tests {
    use std::{
        env, process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use kithara_test_utils::kithara;

    use super::{Geometry, Locate, PathBuf, Rect, Region, Stage, part_file, shoot_part};
    use crate::capture::diff::read_png;

    static DIR_ID: AtomicU64 = AtomicU64::new(0);

    /// A stage that photographs the pixels it was built with, and knows where
    /// one control of them is.
    struct Card {
        frame: Geometry,
        rect: Option<Rect>,
        pixels: Vec<u8>,
    }

    impl Stage for Card {
        type Page = &'static str;

        fn geometry(&self) -> Geometry {
            self.frame
        }

        fn shoot(&mut self) -> Result<&[u8], String> {
            Ok(&self.pixels)
        }

        fn tick(&mut self) {}

        fn turn(&mut self, _page: &Self::Page) -> Result<(), String> {
            Ok(())
        }
    }

    impl Locate for Card {
        fn locate(&self, _path: &str) -> Option<Rect> {
            self.rect
        }
    }

    /// A frame whose every pixel says which one it is, so a picture cut out of
    /// it can be read back as the place it was cut from.
    fn card(rect: Rect) -> Card {
        Card {
            frame: Geometry {
                height: 4,
                scale: 1.0,
                width: 4,
            },
            pixels: (0..4u8)
                .flat_map(|y| (0..4u8).flat_map(move |x| [x, y, 0, 255]))
                .collect(),
            rect: Some(rect),
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { h, w, x, y }
    }

    /// A folder of this run's own, so two tests writing at once cannot read
    /// each other's pictures.
    fn scratch_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "kithara-part-{name}-{}-{}",
            process::id(),
            DIR_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[kithara::test]
    fn a_photograph_of_a_control_carries_the_pixels_it_was_laid_out_over() {
        let dir = scratch_dir("pixels");
        let file = shoot_part(
            &mut card(rect(1.0, 2.0, 2.0, 1.0)),
            &"clock",
            "deck/play",
            &dir,
        )
        .expect("a control the page draws");
        let picture = read_png(&file).expect("a picture that was just written");
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(picture.rgba, [1, 2, 0, 255, 2, 2, 0, 255]);
    }

    #[kithara::test]
    fn a_photograph_of_a_control_lands_under_its_own_name() {
        let dir = scratch_dir("name");
        let file = shoot_part(
            &mut card(rect(1.0, 1.0, 2.0, 2.0)),
            &"clock",
            "deck/play",
            &dir,
        )
        .expect("a control the page draws");
        assert_eq!(file, dir.join("clock-deck-play.png"));
    }

    #[kithara::test]
    fn a_page_that_draws_no_such_control_is_refused() {
        let dir = scratch_dir("missing");
        let mut stage = card(rect(1.0, 1.0, 2.0, 2.0));
        stage.rect = None;
        assert!(shoot_part(&mut stage, &"clock", "deck/play", &dir).is_err());
    }

    #[kithara::test]
    fn a_control_that_leaves_the_frame_is_refused() {
        let dir = scratch_dir("outside");
        assert!(
            shoot_part(
                &mut card(rect(2.0, 0.0, 3.0, 1.0)),
                &"clock",
                "deck/play",
                &dir
            )
            .is_err()
        );
    }

    #[kithara::test]
    fn a_control_too_small_for_the_scale_it_is_photographed_at_is_refused() {
        let dir = scratch_dir("thin");
        let mut stage = card(rect(0.0, 0.0, 1.0, 1.0));
        stage.frame.scale = 0.1;
        assert!(shoot_part(&mut stage, &"clock", "deck/play", &dir).is_err());
    }

    #[kithara::test]
    fn a_frame_the_photograph_does_not_fill_is_refused() {
        let dir = scratch_dir("short");
        let mut stage = card(rect(0.0, 0.0, 2.0, 2.0));
        stage.pixels.truncate(8);
        assert!(shoot_part(&mut stage, &"clock", "deck/play", &dir).is_err());
    }

    #[kithara::test]
    fn a_control_covers_as_many_pixels_as_the_scale_it_is_drawn_at() {
        assert_eq!(
            Region::of(rect(3.0, 1.0, 10.0, 5.0), 2.0).expect("a control inside the frame"),
            Region {
                height: 10,
                width: 20,
                x: 6,
                y: 2,
            }
        );
    }

    #[kithara::test]
    fn a_control_laid_out_before_the_frame_begins_is_refused() {
        assert!(Region::of(rect(-1.0, 1.0, 10.0, 5.0), 1.0).is_err());
    }

    #[kithara::test]
    fn a_control_of_no_size_is_refused() {
        assert!(Region::of(rect(1.0, 1.0, 10.0, 0.0), 1.0).is_err());
    }

    #[kithara::test]
    fn a_photograph_of_a_control_is_named_after_the_page_and_the_control() {
        assert_eq!(
            part_file(&"transport", "gallery/transport/play"),
            "transport-gallery-transport-play.png"
        );
    }
}
