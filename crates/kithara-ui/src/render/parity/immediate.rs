//! The immediate host, kept between events so a gesture can be played to it.
//!
//! The other parity fixtures reach into one widget and hand it an event
//! directly, which is enough for a control that stands in the tree. A surface
//! does not: a popover is an overlay, and only the runtime's own interface
//! builds one. So this mounts the whole document the way a window does, and
//! keeps what the tree remembered between one event and the next.

use std::borrow::Cow;

use iced::{
    Event, Point, Size,
    advanced::{clipboard, graphics::text::font_system, mouse::Cursor},
    mouse::{self, Button},
};
use iced_runtime::{
    UserInterface,
    user_interface::{Cache, State},
};
use num_traits::cast::AsPrimitive;

use super::shared::renderer;
use crate::{
    app::App,
    compile::CompiledUi,
    draw::Pt,
    render::{Clock, ControlAction, Skin, UiEvent, fonts::FONT_BYTES, tree},
    view::ViewState,
};

/// One compiled document, drawn and answered by the immediate host.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(super) struct Immediate<'a, A> {
    /// The application this host is showing.
    #[field(get, vis = "pub(super)")]
    app: A,
    /// What the interface kept of the tree it built last time. An immediate
    /// host forgets the tree between frames, so what a widget remembers of a
    /// gesture - a press it has begun, a surface it has latched - lives here
    /// and nowhere else.
    cache: Cache,
    renderer: iced::Renderer,
    size: Size,
    skin: &'a Skin,
    /// What the tree said of itself when it last answered an event.
    state: State,
    ui: &'a CompiledUi,
    /// The hand the tree last asked the window to show.
    #[field(get(copy), vis = "pub(super)")]
    hand: mouse::Interaction,
    /// The state the screen keeps for itself, which this host owns exactly as
    /// the retained one does.
    #[field(get, vis = "pub(super)")]
    view: ViewState,
}

impl<'a, A: App> Immediate<'a, A> {
    /// Mounts the document, registering the toolkit's own faces with the font
    /// system this host shapes through the way a window does on the way up.
    pub(super) fn mount(app: A, ui: &'a CompiledUi, skin: &'a Skin, size: (u32, u32)) -> Self {
        let mut fonts = font_system()
            .write()
            .unwrap_or_else(|error| panic!("iced font system lock: {error}"));
        for bytes in FONT_BYTES {
            fonts.load_font(Cow::Borrowed(bytes));
        }
        drop(fonts);
        Self {
            app,
            cache: Cache::default(),
            hand: mouse::Interaction::None,
            renderer: renderer(),
            size: Size::new(size.0.as_(), size.1.as_()),
            skin,
            state: State::Outdated,
            ui,
            view: ViewState::default(),
        }
    }

    /// A whole press at one point of the window: the pointer arrives, presses
    /// and lets go, each one its own frame the way a runtime hands them over.
    pub(super) fn click_at(&mut self, at: Pt) {
        let cursor = Point::new(at.x, at.y);
        for event in [
            Event::Mouse(mouse::Event::CursorMoved { position: cursor }),
            Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(Button::Left)),
        ] {
            for published in self.dispatch(cursor, &event) {
                self.settle(published);
            }
        }
    }

    /// The pointer arrives at one point and stops there, pressing nothing.
    pub(super) fn hover_at(&mut self, at: Pt) {
        let cursor = Point::new(at.x, at.y);
        let moved = Event::Mouse(mouse::Event::CursorMoved { position: cursor });
        for published in self.dispatch(cursor, &moved) {
            self.settle(published);
        }
    }

    /// A drag still under way: the pointer arrives, presses, and travels to a
    /// second point without letting go.
    pub(super) fn drag_from(&mut self, at: Pt, to: Pt) {
        let start = Point::new(at.x, at.y);
        let end = Point::new(to.x, to.y);
        for (cursor, event) in [
            (
                start,
                Event::Mouse(mouse::Event::CursorMoved { position: start }),
            ),
            (
                start,
                Event::Mouse(mouse::Event::ButtonPressed(Button::Left)),
            ),
            (
                end,
                Event::Mouse(mouse::Event::CursorMoved { position: end }),
            ),
        ] {
            for published in self.dispatch(cursor, &event) {
                self.settle(published);
            }
        }
    }

    /// Builds the tree, hands it one event, and keeps what the tree remembered
    /// along with the hand it asked for.
    ///
    /// A tree that reports itself outdated has no hand to give: its layout no
    /// longer answers for the pointer. The runtime rebuilds and asks again, and
    /// so does this, with no event the second time so nothing is delivered
    /// twice.
    fn dispatch(&mut self, cursor: Point, event: &Event) -> Vec<UiEvent> {
        let mut published = self.deliver(cursor, std::slice::from_ref(event));
        if matches!(self.state, State::Outdated) {
            published.extend(self.deliver(cursor, &[]));
        }
        published
    }

    /// Builds the tree, hands it the events, and keeps what it remembered.
    fn deliver(&mut self, cursor: Point, events: &[Event]) -> Vec<UiEvent> {
        let Self {
            app,
            cache,
            hand,
            renderer,
            size,
            skin,
            state,
            ui,
            view,
        } = self;
        let element = app
            .reads(|reads| tree::render(&ui.root, ui, reads, view, skin, Clock::default(), None));
        let mut interface = UserInterface::build(element, *size, std::mem::take(cache), renderer);
        let mut published: Vec<UiEvent> = Vec::new();
        let (settled, _) = interface.update(
            events,
            Cursor::Available(cursor),
            renderer,
            &mut clipboard::Null,
            &mut published,
        );
        if let State::Updated {
            mouse_interaction, ..
        } = &settled
        {
            *hand = *mouse_interaction;
        }
        *state = settled;
        *cache = interface.into_cache();
        published
    }

    /// Applies what the press writes to the screen's own state, then tells the
    /// application. The state a document turns for itself belongs to whichever
    /// host is showing it, so this host turns it exactly as the retained one
    /// does before the application hears anything.
    fn settle(&mut self, event: UiEvent) {
        if let UiEvent::Control { path, action } = &event
            && matches!(action, ControlAction::Activate)
            && let Some((state, write)) = self.ui.views().at(path)
        {
            self.view.apply(state, write);
        }
        self.app.update(event);
    }
}
