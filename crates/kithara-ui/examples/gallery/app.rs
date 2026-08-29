use iced::{Element, Size, Task, Theme, theme, window};
use kithara_platform::time::Duration;
use kithara_ui::{
    builtin,
    compile::{CompiledUi, compile},
    render::{Clock, Skin, UiEvent, WindowCommand, custom::CustomKinds, tree},
    skin::SkinDoc,
};

use crate::{
    capture::{Capture, Shot},
    fixture::{Consts, Resolver, resolver},
    mock::MockReads,
    sections::{ModuleDemo, Tab},
};

#[derive(Clone, Debug)]
pub(crate) enum Message {
    Tick,
    Ui(UiEvent),
    /// Move to the next page to photograph, or finish and exit.
    CaptureNext,
    /// The page is on screen; ask the window for its pixels.
    CaptureShoot(Shot),
    /// Write one page to disk.
    CaptureSave(Shot, window::Screenshot),
}

pub(crate) struct Gallery {
    pub(crate) window_id: window::Id,
    /// This host's own reading of time, advanced by the same step the tick
    /// subscription fires at, so a document bound to it moves with the page.
    pub(crate) clock: Clock,
    pub(crate) reads: MockReads,
    /// The extensions this application registers, offered to whichever host
    /// draws the page that names one.
    pub(crate) kinds: CustomKinds,
    pub(crate) layouts: [CompiledUi; Tab::ALL.len()],
    pub(crate) module_layouts: [CompiledUi; ModuleDemo::ALL.len()],
    pub(crate) capture: Option<Capture>,
}

impl Gallery {
    /// The skin the gallery is dressed in, read off the same state every page
    /// is read from, so turning a page cannot undress it.
    pub(crate) fn skin(&self) -> &'static Skin {
        self.reads.skin()
    }

    /// The gallery with no window of iced's: the offscreen capture rasterises
    /// the same documents itself, and never opens one.
    pub(crate) fn mounted() -> Self {
        let resolver = resolver();
        let endpoints = crate::mock::registry();
        let skin = builtin::skin().document();
        Self {
            layouts: Tab::ALL.map(|tab| compiled(tab.entry(), &resolver, &endpoints, skin)),
            module_layouts: ModuleDemo::ALL
                .map(|module| compiled(module.entry(), &resolver, &endpoints, skin)),
            window_id: window::Id::unique(),
            clock: Clock::default(),
            reads: MockReads::default(),
            kinds: crate::custom::kinds(),
            capture: None,
        }
    }

    /// Whether the page the gallery is showing reads differently next frame,
    /// which is the window's whole reason to come back for one.
    ///
    /// Two things move a page and only one of them is written down. The
    /// document declares its own motion, and the compiled page carries that
    /// declaration; the application also hands over readings it moved itself,
    /// which no document can declare because the application decides them one
    /// frame at a time. A window that honoured the declaration alone froze the
    /// second kind between unrelated events.
    pub(crate) fn moves(&self) -> bool {
        self.compiled().animates || self.reads.feeds()
    }

    /// One frame of the gallery's own time: the clock a document binds to,
    /// and the application's own reading of how far along it is.
    pub(crate) fn tick(&mut self) {
        self.clock = self
            .clock
            .advance(Duration::from_millis(Consts::STRESS_TICK_MS));
        self.reads.tick();
    }

    /// Turns to the page a shot names, as freshly as the retained host mounts
    /// one: that host builds a page its own, so a page opens here at nothing
    /// on the clock and nothing behind it. Carrying the page before it over
    /// would photograph the two hosts at two different moments, and a film of
    /// a page would open wherever the page before it left off.
    pub(crate) fn select(&mut self, shot: Shot) {
        self.clock = Clock::default();
        self.reads = MockReads::default();
        self.reads.select_tab(shot.tab);
        if let Some(module) = shot.module {
            self.reads.select_module(module);
        }
    }

    pub(crate) fn compiled(&self) -> &CompiledUi {
        if self.reads.active_tab() == Tab::Modules {
            &self.module_layouts[self.reads.active_module().index()]
        } else {
            &self.layouts[self.reads.active_tab().index()]
        }
    }

    /// Compiles every page again against the skin the gallery has turned to.
    ///
    /// A document is compiled against a skin and not merely painted with one:
    /// what a page measures comes from the skin's own numbers, so a skin
    /// changed at runtime is a set of pages built again.
    pub(crate) fn dress(&mut self) {
        let resolver = resolver();
        let endpoints = crate::mock::registry();
        let skin = self.skin().document();
        self.layouts = Tab::ALL.map(|tab| compiled(tab.entry(), &resolver, &endpoints, skin));
        self.module_layouts =
            ModuleDemo::ALL.map(|module| compiled(module.entry(), &resolver, &endpoints, skin));
    }

    /// Selects the next page and lets one frame render before the shot.
    fn capture_next(&mut self) -> Task<Message> {
        let Some(capture) = self.capture.as_mut() else {
            return Task::none();
        };
        let Some(shot) = capture.next() else {
            capture.report();
            return iced::exit();
        };
        self.select(shot);
        Task::done(Message::CaptureShoot(shot))
    }

    fn capture_save(&mut self, shot: Shot, image: &window::Screenshot) -> Task<Message> {
        let Some(capture) = self.capture.as_mut() else {
            return Task::none();
        };
        match capture.save(shot, image) {
            Ok(path) => println!("captured {} ({} left)", path.display(), capture.remaining()),
            Err(error) => eprintln!("capture failed: {error}"),
        }
        Task::done(Message::CaptureNext)
    }
}

pub(crate) fn update(state: &mut Gallery, message: Message) -> Task<Message> {
    match message {
        Message::Tick => {
            state.tick();
            Task::none()
        }
        Message::Ui(UiEvent::Control { path, action }) => {
            if let Ok(tab) = Tab::try_from(path.as_str()) {
                state.reads.select_tab(tab);
            } else {
                let was = state.reads.active_skin();
                state.reads.apply(&path, &action);
                if state.reads.active_skin() != was {
                    state.dress();
                }
            }
            Task::none()
        }
        Message::Ui(UiEvent::LibraryQuery(query)) => {
            state.reads.set_library_query(query);
            Task::none()
        }
        Message::Ui(UiEvent::ToggleModule(module)) => {
            state.reads.toggle_module(module);
            Task::none()
        }
        Message::Ui(UiEvent::Window(command)) => match command {
            WindowCommand::Drag => window::drag(state.window_id),
            WindowCommand::Minimize => window::minimize(state.window_id, true),
            WindowCommand::ToggleMaximize => window::toggle_maximize(state.window_id),
            WindowCommand::Close => iced::exit(),
            _ => Task::none(),
        },
        Message::Ui(_) => Task::none(),
        Message::CaptureNext => state.capture_next(),
        Message::CaptureShoot(shot) => {
            window::screenshot(state.window_id).map(move |image| Message::CaptureSave(shot, image))
        }
        Message::CaptureSave(shot, image) => state.capture_save(shot, &image),
    }
}

pub(crate) fn view(state: &Gallery, _window: window::Id) -> Element<'_, Message> {
    tree::render(
        &state.compiled().root,
        state.compiled(),
        &state.reads,
        state.skin(),
        state.clock,
        Some(&state.kinds),
    )
    .map(Message::Ui)
}

/// The window every capture is taken at, so the three sets line up without
/// anyone stating the geometry twice.
pub(crate) fn window_size() -> Size {
    Size::new(Consts::WIDTH, Consts::HEIGHT)
}

fn compiled(
    entry: &str,
    resolver: &Resolver,
    endpoints: &dyn kithara_ui::registry::EndpointRegistry,
    skin: &SkinDoc,
) -> CompiledUi {
    compile(
        entry,
        resolver,
        endpoints,
        skin,
        builtin::text_doc(),
        crate::custom::config(),
    )
    .unwrap_or_else(|error| panic!("embedded gallery document {entry} must compile: {error}"))
}

pub(crate) fn theme(skin: &Skin) -> Theme {
    let palette = skin.palette;
    Theme::custom(
        "Kithara".to_owned(),
        theme::Palette {
            background: palette.bg.into(),
            text: palette.text.into(),
            primary: palette.accent.into(),
            success: palette.success.into(),
            danger: palette.danger.into(),
            warning: palette.warning.into(),
        },
    )
}
