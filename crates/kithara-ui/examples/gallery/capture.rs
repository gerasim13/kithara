use std::{
    env,
    fmt::{self, Display},
    fs::create_dir_all,
    path::PathBuf,
};

use iced::window::Screenshot;
use kithara_ui::capture::{Film, Geometry, page_file, write_geometry, write_png};

use crate::sections::{ModuleDemo, Tab};

/// One page to photograph: a tab, and for the modules tab the demo shown in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Shot {
    pub(super) tab: Tab,
    pub(super) module: Option<ModuleDemo>,
}

impl Shot {
    /// Every gallery page in tab order, with the modules tab expanded per demo.
    pub(super) fn all() -> Vec<Self> {
        let mut pages = Vec::new();
        for tab in Tab::ALL {
            if tab == Tab::Modules {
                pages.extend(ModuleDemo::ALL.map(|module| Self {
                    tab,
                    module: Some(module),
                }));
            } else {
                pages.push(Self { tab, module: None });
            }
        }
        pages
    }
}

/// The page's own name, which is the slug its nav item, its document and its
/// reading already answer to. It carried the page's position before, so
/// inserting a page renamed every page after it and left the parity budget
/// pricing files nothing writes.
impl Display for Shot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tab = self.tab.slug();
        match self.module {
            None => f.write_str(tab),
            Some(module) => write!(f, "{tab}-{}", module.slug()),
        }
    }
}

/// The film this run asked for, if it asked for one.
///
/// Named by `KITHARA_GALLERY_CAPTURE_FILM=<pages>:<photos>x<steps>`, as in
/// `motion,sprites,lottie:16x15`. A page whose document moves draws a
/// different picture at every photograph, which a single still cannot show.
///
/// `Ok(None)` means no film was asked for; a spec that cannot be read ends the
/// capture rather than quietly becoming a still set the caller did not ask for.
pub(super) fn requested() -> Result<Option<Film<Shot>>, String> {
    let Some(spec) = env::var_os("KITHARA_GALLERY_CAPTURE_FILM") else {
        return Ok(None);
    };
    let spec = spec
        .to_str()
        .ok_or_else(|| "KITHARA_GALLERY_CAPTURE_FILM is not text".to_owned())?;
    parse(spec).map(Some)
}

/// Every page once, which is what a capture with no film asked for takes.
pub(super) fn stills() -> Film<Shot> {
    Film::stills(Shot::all())
}

fn parse(spec: &str) -> Result<Film<Shot>, String> {
    let (pages, cadence) = spec
        .split_once(':')
        .ok_or_else(|| format!("expected <pages>:<photos>x<steps>, got {spec}"))?;
    let (photos, steps) = cadence
        .split_once('x')
        .ok_or_else(|| format!("expected <photos>x<steps>, got {cadence}"))?;
    Film::new(
        pages.split(',').map(page).collect::<Result<_, _>>()?,
        count(photos, "photographs")?,
        count(steps, "frames between photographs")?,
    )
}

/// One page of the gallery, named the way its document is.
fn page(name: &str) -> Result<Shot, String> {
    Tab::try_from(format!("gallery/{name}/item").as_str())
        .map(|tab| Shot { tab, module: None })
        .map_err(|()| format!("no gallery page named {name}"))
}

/// A number the spec carries. What counts as too few of it is the film's to
/// say, so this only reads it.
fn count(text: &str, what: &str) -> Result<usize, String> {
    text.parse()
        .map_err(|_| format!("{what} must be a number, got {text}"))
}

/// Walks every gallery page once, writing a PNG per page.
///
/// Enabled by `KITHARA_GALLERY_CAPTURE=<dir>`; the gallery runs normally when
/// the variable is absent, so the same binary serves both uses.
pub(super) struct Capture {
    dir: PathBuf,
    pending: Vec<Shot>,
    written: Vec<PathBuf>,
}

impl Capture {
    /// Reads the environment and builds the page list, newest page last.
    pub(super) fn requested() -> Option<Self> {
        let dir = PathBuf::from(env::var_os("KITHARA_GALLERY_CAPTURE")?);
        let mut pending = Shot::all();
        pending.reverse();
        Some(Self {
            dir,
            pending,
            written: Vec::new(),
        })
    }

    delegate::delegate! {
        to self.pending {
            #[call(pop)]
            pub(super) fn next(&mut self) -> Option<Shot>;
            #[call(len)]
            pub(super) fn remaining(&self) -> usize;
        }
    }

    /// Encodes one RGBA screenshot and records where it landed.
    pub(super) fn save(&mut self, shot: Shot, screenshot: &Screenshot) -> Result<PathBuf, String> {
        create_dir_all(&self.dir)
            .map_err(|error| format!("create {}: {error}", self.dir.display()))?;
        write_geometry(
            &self.dir,
            Geometry {
                height: screenshot.size.height,
                scale: f64::from(screenshot.scale_factor),
                width: screenshot.size.width,
            },
        )?;
        let path = self.dir.join(page_file(&shot, None));
        write_png(
            &path,
            &screenshot.rgba,
            screenshot.size.width,
            screenshot.size.height,
        )?;
        self.written.push(path.clone());
        Ok(path)
    }

    /// One line per page, then the directory — printed when the walk finishes.
    pub(super) fn report(&self) {
        for path in &self.written {
            println!("{}", path.display());
        }
        println!(
            "{} page(s) written to {}",
            self.written.len(),
            self.dir.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{Film, Shot, Tab, parse, stills};

    fn film(spec: &str) -> Film<Shot> {
        parse(spec).expect("the spec is well formed")
    }

    fn shot(tab: Tab) -> Shot {
        Shot { tab, module: None }
    }

    #[kithara::test]
    fn a_film_photographs_the_pages_it_names_in_the_order_it_names_them() {
        assert_eq!(
            film("motion,lottie:2x3").pages,
            vec![shot(Tab::Motion), shot(Tab::Lottie)]
        );
    }

    #[kithara::test]
    fn a_film_takes_as_many_photographs_as_it_asks_for() {
        assert_eq!(film("motion:2x3").photos, 2);
    }

    #[kithara::test]
    fn a_film_runs_as_many_frames_between_photographs_as_it_asks_for() {
        assert_eq!(film("motion:2x3").steps, 3);
    }

    #[kithara::test]
    fn a_page_the_gallery_does_not_have_is_refused() {
        assert!(parse("nowhere:2x3").is_err());
    }

    #[kithara::test]
    fn a_spec_with_no_cadence_is_refused() {
        assert!(parse("motion").is_err());
    }

    #[kithara::test]
    fn a_cadence_with_no_frame_count_is_refused() {
        assert!(parse("motion:2").is_err());
    }

    #[kithara::test]
    fn a_film_of_no_photographs_is_refused() {
        assert!(parse("motion:0x3").is_err());
    }

    #[kithara::test]
    fn a_film_with_no_time_between_photographs_is_refused() {
        assert!(parse("motion:2x0").is_err());
    }

    #[kithara::test]
    fn a_page_photographed_once_keeps_the_name_a_still_set_gives_it() {
        assert_eq!(film("motion:1x1").file(&shot(Tab::Motion), 0), "motion.png");
    }

    #[kithara::test]
    fn a_page_photographed_more_than_once_numbers_its_photographs() {
        assert_eq!(
            film("motion:2x1").file(&shot(Tab::Motion), 1),
            "motion-001.png"
        );
    }

    #[kithara::test]
    fn a_still_set_is_every_page_once() {
        let stills = stills();
        assert_eq!(stills.pages, Shot::all());
    }

    #[kithara::test]
    fn a_still_set_takes_one_photograph_of_each_page() {
        assert_eq!(stills().photos, 1);
    }
}
