use std::{
    fmt::{self, Display},
    fs::create_dir_all,
    path::PathBuf,
    str::FromStr,
};

use iced::window::Screenshot;
use kithara_ui::capture::{Geometry, page_file, write_geometry, write_png};

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

impl FromStr for Shot {
    type Err = String;

    /// A page named the way its document is, which is how a run asks for one.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Tab::try_from(format!("gallery/{name}/item").as_str())
            .map(|tab| Self { tab, module: None })
            .map_err(|()| format!("no gallery page named {name}"))
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

/// Walks every gallery page once through a window, writing a PNG per page.
///
/// Asked for by `--shoot <dir> --windowed`. It is the one capture that needs a
/// display, and its job is to say whether the off-screen sets draw what a
/// window draws.
pub(super) struct Capture {
    dir: PathBuf,
    pending: Vec<Shot>,
    written: Vec<PathBuf>,
}

impl Capture {
    /// Builds the page list, newest page last.
    pub(super) fn new(dir: PathBuf) -> Self {
        let mut pending = Shot::all();
        pending.reverse();
        Self {
            dir,
            pending,
            written: Vec::new(),
        }
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
