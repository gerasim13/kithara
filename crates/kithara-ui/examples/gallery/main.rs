//! The gallery: one program that reads its pages from disk and shows them.
//!
//! What the pages are made of is the toolkit's; what checks them is
//! `tests/gallery.rs`, which mounts these same modules. This file is only the
//! window and the harnesses that stand in front of it, each named by a flag of
//! its own — `--help` lists them.

mod app;
mod capture;
mod cli;
mod compare;
mod custom;
mod demo;
mod fixture;
#[cfg(feature = "masonry")]
mod host;
mod offscreen;
#[cfg(feature = "masonry")]
mod retained;
mod sections;

use clap::Parser;
use iced::{
    Color, Subscription, Task, Theme, theme, theme::Base, time as iced_time, window,
    window::Settings,
};
use kithara_ui::render::{UiEvent, WindowCommand, fonts};

use self::{
    app::{Gallery, Message},
    capture::Capture,
    cli::{Args, Host},
};

/// What a run was asked for, once its flags have been read.
enum Asked {
    /// Nothing but the gallery: show it.
    Gallery,
    /// A harness ran and said what it found; the program is done.
    Done,
}

/// Shows the gallery, unless a flag asked for one of the harnesses standing in
/// front of it: a two-host comparison, or a set of photographs.
fn main() -> iced::Result {
    let args = Args::parse();
    match harness(&args) {
        Ok(Asked::Done) => Ok(()),
        Ok(Asked::Gallery) => gallery(&args),
        Err(error) => refuse(&error),
    }
}

/// Ends the program on something it was asked for and cannot do, saying what.
///
/// A gate reads the exit code; iced's error type has no shape for "the two
/// hosts disagree", and inventing one would say less.
fn refuse(error: &str) -> ! {
    eprintln!("{error}");
    std::process::exit(1)
}

/// Runs whichever harness the flags named, or says the gallery itself was
/// asked for.
///
/// A capture through a window is the one that answers `Gallery`: it walks the
/// pages from inside the window it photographs, so the window has to open.
fn harness(args: &Args) -> Result<Asked, String> {
    if let Some(sets) = args.compare.as_deref() {
        return if compare::run(sets, args.budget.as_deref())? {
            Ok(Asked::Done)
        } else {
            Err("the two sets differ by more than their budget allows".to_owned())
        };
    }
    let Some(dir) = args.shoot.as_deref() else {
        return Ok(Asked::Gallery);
    };
    if args.windowed {
        return match args.host {
            Host::Immediate => Ok(Asked::Gallery),
            #[cfg(feature = "masonry")]
            Host::Retained => Err(
                "the retained host photographs off-screen; drop --windowed to use it".to_owned(),
            ),
        };
    }
    let written = match args.host {
        Host::Immediate if args.element.is_some() => {
            return Err(
                "the immediate host forgets the controls it draws; use --host retained".to_owned(),
            );
        }
        Host::Immediate => offscreen::run(args, dir)?,
        #[cfg(feature = "masonry")]
        Host::Retained => retained::shoot(args, dir)?,
    };
    println!("{written} picture(s) written to {}", dir.display());
    Ok(Asked::Done)
}

/// The gallery itself, through whichever host was named.
fn gallery(args: &Args) -> iced::Result {
    match args.host {
        Host::Immediate => immediate(args),
        #[cfg(feature = "masonry")]
        Host::Retained => retained::show(args).map_or_else(|error| refuse(&error), Ok),
    }
}

/// The gallery with a window of iced's in front of it.
fn immediate(args: &Args) -> iced::Result {
    let start = args.clone();
    let daemon = iced::daemon(move || mount(&start), app::update, app::view)
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

/// The pages are the ones the mounted state compiles, so the program and what
/// checks it open on the same gallery.
fn mount(args: &Args) -> (Gallery, Task<Message>) {
    let settings = Settings {
        size: args.size.into(),
        min_size: Some(args.min_size.into()),
        decorations: false,
        exit_on_close_request: false,
        transparent: true,
        ..Settings::default()
    };
    let (window_id, open) = window::open(settings);
    let capture = args
        .shoot
        .clone()
        .filter(|_| args.windowed)
        .map(Capture::new);
    let start = if capture.is_some() {
        Task::done(Message::CaptureNext)
    } else {
        Task::none()
    };
    let gallery = Gallery {
        window_id,
        capture,
        step: args.step(),
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
        Subscription::batch([close, iced_time::every(state.step).map(|_| Message::Tick)])
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
