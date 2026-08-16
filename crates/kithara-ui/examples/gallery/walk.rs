//! Every page of the gallery, walked with nothing touching it.
//!
//! A window drives itself: it asks for a frame, draws it, and asks again for as
//! long as something is moving. Two ways that goes wrong are invisible to a
//! screenshot, because a single frame of either is the right picture.
//!
//! A page that asks forever while nothing on it moves is an empty spin: the
//! window never sleeps, a core burns, and every other page pays for it in the
//! same event loop. A page that moves and stops asking is worse to use than to
//! measure: it animates only while some unrelated event wakes the window, so it
//! moves under a travelling mouse and freezes the moment the mouse stops.
//!
//! What a page does is checked against what its document says it will do. That
//! answer is one walk of the compiled tree — an object placed by an endpoint, a
//! control that paints every frame of its own accord, or a binding that reads
//! the host's own clock — and it is spelled out per control, so a control added
//! tomorrow is classified here or does not compile.
//!
//! Comparing two pictures instead was tried first and is not sound: six pages
//! rasterise two different images of the same moment, so the oracle measured
//! the rasteriser as much as the page and the set of pages said to move changed
//! between runs of the same binary.
//!
//! The page list is [`Shot::all`], the same one the captures walk, so a page
//! that is photographed is a page that is walked.

use kithara_platform::time::Duration;
use kithara_ui::{
    app::{App, Config, Ui},
    builtin,
    compile::compile,
    source::UiConfig,
};

use super::{Consts, capture::Shot, host::Gallery, mock, resolver};

/// How many frames a page is given to stop asking.
///
/// A page that settles at all settles in one or two: the retained tree lays
/// out, paints, and goes quiet. This is far past that, so a page still asking
/// here is asking forever rather than finishing late.
const FRAMES: usize = 32;

/// What one page did when it was left alone.
struct Walked {
    /// Whether the window would still be drawing this page unprompted.
    asking: bool,
    /// Whether the page's document says it draws a different picture later.
    animates: bool,
}

/// Steps one page with nothing touching it.
///
/// This is the window's own loop with the window taken off, and it has to be
/// exactly that loop or it measures nothing: the window draws only when the
/// tree says the next frame would differ, and asks for another only when
/// finishing this one said to. Miss either condition and a page that has
/// stopped driving itself still looks alive here, because the harness kept
/// calling it.
fn walk(page: Shot) -> Walked {
    let endpoints = mock::registry();
    let resolver = resolver();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .skin(builtin::skin())
        .skin_doc(builtin::skin_doc())
        .build();
    let size = (
        num_traits::cast::AsPrimitive::<u32>::as_(Consts::WIDTH),
        num_traits::cast::AsPrimitive::<u32>::as_(Consts::HEIGHT),
    );
    let at = Gallery::at(page);
    // Read from the document rather than from the host, so the two sides of the
    // comparison come from two places: what the page says, and what the window
    // then does about it.
    let animates = compile(
        at.document(),
        &resolver,
        &endpoints,
        builtin::skin_doc(),
        &UiConfig::default(),
    )
    .unwrap_or_else(|error| panic!("page {} must compile: {error}", page.name()))
    .animates;
    let mut ui = Ui::new(at, config, size, 1.0)
        .unwrap_or_else(|error| panic!("page {} must mount: {error}", page.name()));
    // One frame at sixty a second, which is what the window tells the pass.
    let frame = Duration::from_millis(16);
    // The window asks for the first frame itself, once the surface is up.
    let mut asking = true;
    for _ in 0..FRAMES {
        ui.frame(frame);
        if !ui.needs_frame() {
            asking = false;
            break;
        }
        ui.render()
            .unwrap_or_else(|error| panic!("page {} must draw: {error}", page.name()));
        if !ui.complete_frame() {
            asking = false;
            break;
        }
    }

    Walked { asking, animates }
}

/// The empty spin: the window never sleeps and nothing on the page moves.
#[kithara_test_utils::kithara::test]
fn no_page_asks_for_frames_its_document_never_declared() {
    let spinning: Vec<String> = Shot::all()
        .into_iter()
        .filter(|page| {
            let walked = walk(*page);
            walked.asking && !walked.animates
        })
        .map(Shot::name)
        .collect();

    assert!(
        spinning.is_empty(),
        "these pages ask the window for a frame forever with nothing on them declared to move, so \
         the loop never sleeps and every other page waits behind them: {spinning:?}"
    );
}

/// The other direction: a page that says it moves and stops asking has stopped
/// animating, and only an unrelated event will move it again.
#[kithara_test_utils::kithara::test]
fn every_page_that_declares_motion_keeps_asking_for_frames() {
    let stalled: Vec<String> = Shot::all()
        .into_iter()
        .filter(|page| {
            let walked = walk(*page);
            walked.animates && !walked.asking
        })
        .map(Shot::name)
        .collect();

    assert!(
        stalled.is_empty(),
        "these pages say something on them moves and then stop asking the window for frames, so \
         they animate only while something unrelated keeps waking it — a mouse crossing the \
         window, and nothing when it stops: {stalled:?}"
    );
}
