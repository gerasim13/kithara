use std::{
    fmt::{self, Display},
    fs::create_dir_all,
    iter::once,
    path::PathBuf,
    str::FromStr,
};

use iced::window::Screenshot;
use kithara_ui::{
    capture::{Geometry, page_file, write_geometry, write_png},
    module::ViewSet,
    view::ViewState,
};

use crate::sections::{self, Page};

/// The state each page stands open to show what that page is about, by the name
/// the page's own document gave it.
///
/// A photographer opens a surface to photograph it, where a reader opens it by
/// pressing the control that turns it. Both hosts are handed this the same way,
/// so neither can photograph a page the other one left shut.
const DEMONSTRATED: [(Page, &str); 2] = [
    ("menu", "app-menu/menu"),
    ("clock", "clock-components/clock"),
];

/// One page to photograph: a tab, and for the modules tab the demo shown in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Shot {
    pub(super) module: Option<Page>,
    pub(super) tab: Page,
}

impl Shot {
    /// Which screen states this shot stands, and where: the page the nav
    /// turns, and for the modules page the demo it stands at.
    pub(super) fn stands(&self) -> impl Iterator<Item = (&'static str, Page)> {
        [
            (sections::PAGE, Some(self.tab)),
            (sections::MODULE, self.module),
        ]
        .into_iter()
        .filter_map(|(state, page)| page.map(|page| (state, page)))
    }

    /// Which screen states this shot stands open: the surface the page it
    /// photographs is about, and nothing on a page that is about no surface.
    pub(super) fn opens(&self) -> impl Iterator<Item = &'static str> {
        let tab = self.tab;
        DEMONSTRATED
            .into_iter()
            .filter(move |(page, _)| *page == tab)
            .map(|(_, state)| state)
    }

    /// The screen's own state standing at this page, which is how a harness
    /// opens one page of the screen every page lives in.
    pub(super) fn standing(&self) -> ViewState {
        let mut view = ViewState::default();
        for (state, page) in self.stands() {
            view.stand(state, page);
        }
        for state in self.opens() {
            view.set(state, ViewSet::On);
        }
        view
    }

    /// Every gallery page in tab order, with the modules tab expanded per demo.
    pub(super) fn all() -> Vec<Self> {
        let mut pages = Vec::new();
        for tab in sections::pages().iter().copied() {
            if tab == sections::MODULES {
                pages.extend(sections::modules().iter().copied().map(|module| Self {
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
        sections::named(name)
            .map(|tab| Self { tab, module: None })
            .ok_or_else(|| format!("no gallery page named {name}"))
    }
}

/// The page's own name, which is the slug its nav item, its document and its
/// reading already answer to. It carried the page's position before, so
/// inserting a page renamed every page after it and left the parity budget
/// pricing files nothing writes.
impl Display for Shot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tab = self.tab;
        match self.module {
            None => f.write_str(tab),
            Some(module) => write!(f, "{tab}-{module}"),
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
            screenshot.size.width,
            screenshot.size.height,
            once(screenshot.rgba.as_ref()),
        )?;
        self.written.push(path.clone());
        Ok(path)
    }

    delegate::delegate! {
        to self.pending {
            #[call(pop)]
            pub(super) fn next(&mut self) -> Option<Shot>;
            #[call(len)]
            pub(super) fn remaining(&self) -> usize;
        }
    }
}
