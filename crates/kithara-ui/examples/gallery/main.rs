//! The gallery: one program that reads its pages from disk and shows them.
//!
//! What the pages are made of is the toolkit's; what checks them is
//! `tests/gallery.rs`, which mounts these same modules. This file is only the
//! window and the harnesses that stand in front of it.

mod app;
mod capture;
mod compare;
mod custom;
mod fixture;
#[cfg(feature = "masonry")]
mod host;
#[cfg(feature = "masonry")]
mod masonry_shots;
mod mock;
mod offscreen;
mod sections;

use iced::{
    Color, Size, Subscription, Task, Theme, theme, theme::Base, time as iced_time, window,
    window::Settings,
};
use kithara_platform::time::Duration;
use kithara_ui::render::{UiEvent, WindowCommand, fonts};

use self::{
    app::{Gallery, Message},
    capture::Capture,
    fixture::Consts,
};

/// Opens the gallery, unless one of the harnesses standing in front of it was
/// asked for instead: a two-host comparison, a still set, or one of the
/// retained host's own runs.
fn main() -> iced::Result {
    match compare::run() {
        compare::Verdict::Passed => return Ok(()),
        // A gate says so with its exit code; iced's error type has no shape for
        // "the two hosts disagree", and inventing one would say less.
        compare::Verdict::Failed => std::process::exit(1),
        compare::Verdict::NotAsked => {}
    }
    if offscreen::run() {
        return Ok(());
    }
    #[cfg(feature = "masonry")]
    if masonry_shots::run() || host::run() {
        return Ok(());
    }
    let daemon = iced::daemon(new, app::update, app::view)
        .title(|_state: &Gallery, _window| "Kithara UI Gallery".to_owned())
        .theme(|state: &Gallery, _window| app::theme(state.skin()))
        .style(|_state: &Gallery, theme: &Theme| window_style(theme))
        .subscription(subscription)
        .default_font(fonts::SANS);
    fonts::FONT_BYTES
        .iter()
        .fold(daemon, |daemon, bytes| daemon.font(*bytes))
        .run()
}

/// The gallery with a window of iced's in front of it. The pages are the ones
/// the mounted state compiles, so the program and what checks it open on the
/// same gallery.
fn new() -> (Gallery, Task<Message>) {
    let settings = Settings {
        size: Size::new(Consts::WIDTH, Consts::HEIGHT),
        min_size: Some(Size::new(Consts::MIN_WIDTH, Consts::MIN_HEIGHT)),
        decorations: false,
        exit_on_close_request: false,
        transparent: true,
        ..Settings::default()
    };
    let (window_id, open) = window::open(settings);
    let capture = Capture::requested();
    let start = if capture.is_some() {
        Task::done(Message::CaptureNext)
    } else {
        Task::none()
    };
    let gallery = Gallery {
        window_id,
        capture,
        ..Gallery::mounted()
    };
    (gallery, open.discard().chain(start))
}

/// Time runs on the pages that move, which the gallery answers for itself.
/// Naming the pages here instead is a second account of the same fact, and it
/// drifts: a page that gained something moving kept its picture frozen until an
/// unrelated event redrew it, and one that lost it went on waking the host
/// every tick for nothing.
///
/// A capture never ticks: the offscreen host photographs one frame of a freshly
/// mounted page, so a clock running here would put the two hosts at different
/// moments and the comparison would measure the difference between them.
///
/// A close request arrives as the window command the title bar's own button
/// sends, so the two ways to shut the gallery meet in one arm rather than
/// ending the program from two places.
fn subscription(state: &Gallery) -> Subscription<Message> {
    let close =
        window::close_requests().map(|_| Message::Ui(UiEvent::Window(WindowCommand::Close)));
    if state.capture.is_none() && state.moves() {
        Subscription::batch([
            close,
            iced_time::every(Duration::from_millis(Consts::STRESS_TICK_MS)).map(|_| Message::Tick),
        ])
    } else {
        close
    }
}

/// The window paints no ground of its own: the document lays down the page in
/// the shape the skin gives the window, and whatever the shape leaves out is
/// the desktop behind it.
fn window_style(theme: &Theme) -> theme::Style {
    theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: theme.base().text_color,
    }
}
