//! The gallery pressed through the tree its window draws.

use std::mem;

use iced::{
    Event, Point, Size,
    advanced::{clipboard, mouse::Cursor},
    mouse::{self, Button},
    window,
};
use iced_runtime::user_interface::{Cache, UserInterface};
use kithara_test_utils::kithara;
use kithara_ui::{
    module::ViewSet,
    render::{ControlAction, Published},
};

use super::walk::immediate;
use crate::{
    app::{self, Gallery, Message},
    capture::Shot,
    sections::Page,
};

const STEP_Y: f32 = 20.0;
const STEP_X: f32 = 60.0;

fn published(
    interface: &mut UserInterface<'_, Message, iced::Theme, iced::Renderer>,
    pressed: &[Event],
    at: Point,
    renderer: &mut iced::Renderer,
) -> Vec<Published> {
    let mut messages = Vec::new();
    let _ = interface.update(
        pressed,
        Cursor::Available(at),
        renderer,
        &mut clipboard::Null,
        &mut messages,
    );
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::Ui(published) => Some(published),
            _ => None,
        })
        .collect()
}

struct Window {
    gallery: Gallery,
    renderer: iced::Renderer,
    cache: Cache,
}

impl Window {
    fn at(tab: Page) -> Self {
        let mut gallery = Gallery::mounted();
        gallery.select(Shot { tab, module: None });
        Self {
            gallery,
            renderer: immediate::renderer(),
            cache: Cache::default(),
        }
    }

    fn press(&mut self, button: Button, at: Point) -> Vec<Published> {
        let published = self.press_once(button, at);
        for event in &published {
            drop(app::update(&mut self.gallery, Message::Ui(event.clone())));
        }
        published
    }

    fn find<const N: usize>(
        &mut self,
        button: Button,
        action: &ControlAction,
        paths: [&str; N],
    ) -> [Vec<Point>; N] {
        let mut found = [const { Vec::new() }; N];
        self.each_press(button, |at, published| {
            for (path, points) in paths.iter().zip(&mut found) {
                if published.iter().any(|event| {
                    matches!(event, Published::Gesture { path: on, action: done }
                        if on == path && done == action)
                }) {
                    points.push(at);
                }
            }
        });
        for (path, points) in paths.iter().zip(&found) {
            assert!(!points.is_empty(), "no point of the window presses {path}");
        }
        found
    }

    fn each_press(&mut self, button: Button, mut visit: impl FnMut(Point, &[Published])) {
        let logical = Size::from(crate::cli::WINDOW);
        let mut interface = UserInterface::build(
            app::view(&self.gallery, window::Id::unique()),
            logical,
            mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let pressed = [Event::Mouse(mouse::Event::ButtonPressed(button))];
        let rows = (0..)
            .map(|row: u16| f32::from(row) * STEP_Y)
            .take_while(|y| *y < logical.height);
        for y in rows {
            let columns = (0..)
                .map(|column: u16| f32::from(column) * STEP_X)
                .take_while(|x| *x < logical.width);
            for x in columns {
                let at = Point::new(x, y);
                let published = published(&mut interface, &pressed, at, &mut self.renderer);
                visit(at, &published);
            }
        }
        self.cache = interface.into_cache();
    }

    fn press_once(&mut self, button: Button, at: Point) -> Vec<Published> {
        let logical = Size::from(crate::cli::WINDOW);
        let mut interface = UserInterface::build(
            app::view(&self.gallery, window::Id::unique()),
            logical,
            mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let pressed = [Event::Mouse(mouse::Event::ButtonPressed(button))];
        let published = published(&mut interface, &pressed, at, &mut self.renderer);
        self.cache = interface.into_cache();
        published
    }

    fn open_menus(&self) -> Vec<u8> {
        (1..=4)
            .filter(|row| self.gallery.view.flag(&format!("ctx/{row}")))
            .collect()
    }
}

#[kithara::test]
fn a_secondary_press_on_another_track_only_dismisses_the_open_menu() {
    let mut window = Window::at("menu");
    // The page is photographed with the app menu standing open over it.
    window.gallery.view.set("app-menu/menu", ViewSet::Off);
    let [rows_1, rows_2] = window.find(
        Button::Right,
        &ControlAction::SecondaryActivate,
        ["ctx/track-1/row", "ctx/track-2/row"],
    );
    let first = rows_1[0];
    // The menu opens at the pointer, so row 2 is pressed as far from it as the
    // row reaches.
    let second = rows_2
        .into_iter()
        .max_by(|a, b| (a.x - first.x).abs().total_cmp(&(b.x - first.x).abs()))
        .unwrap_or(first);

    window.press(Button::Right, first);
    assert_eq!(window.open_menus(), [1]);

    let published = window.press(Button::Right, second);

    assert!(
        !published.iter().any(|event| matches!(
            event,
            Published::Gesture { path, .. } if path == "ctx/track-2/row"
        )),
        "the press must be absorbed by the open menu, published {published:?}"
    );
    assert!(!window.gallery.view.flag("ctx/2"));
    assert_eq!(
        window.open_menus(),
        Vec::<u8>::new(),
        "the press dismissed the open menu, published {published:?}"
    );
}
