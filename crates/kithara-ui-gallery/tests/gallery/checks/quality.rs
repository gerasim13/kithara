//! What the quality cell reports and what picking a variant does to it.

use kithara_test_utils::kithara;
use kithara_ui::render::{ReadValue, Reads as _};

use super::Hand;
use crate::DemoReads;

const CELL: &str = "modules/deck/transport/stream/cell";
const MENU: &str = "modules/quality";
fn text(state: &DemoReads, endpoint: &str) -> String {
    match state.get(endpoint) {
        Some(ReadValue::Text(value)) => value.to_owned(),
        other => panic!("{endpoint} must read as text, got {other:?}"),
    }
}

fn flag(state: &DemoReads, endpoint: &str) -> bool {
    match state.get(endpoint) {
        Some(ReadValue::Bool(value)) => value,
        other => panic!("{endpoint} must read as a flag, got {other:?}"),
    }
}

#[kithara::test]
fn the_cell_names_the_variant_the_ladder_plays_while_it_picks_them() {
    let state = DemoReads::default();

    assert_eq!(text(&state, "deck.stream.quality@deck=a"), "AUTO·320");
    assert!(flag(
        &state,
        "deck.stream.variant_active@deck=a,variant=auto"
    ));
    assert!(!flag(&state, "deck.stream.variant_active@deck=a,variant=1"));
}

#[kithara::test]
fn picking_a_variant_leaves_auto_and_closes_the_menu() {
    let mut hand = Hand::at("modules");
    hand.press(CELL);
    assert!(hand.view.flag(MENU));

    hand.press("modules/deck/transport/stream/variant-2/pick");

    let state = &hand.reads;
    assert_eq!(text(state, "deck.stream.quality@deck=a"), "128");
    assert!(!flag(
        state,
        "deck.stream.variant_active@deck=a,variant=auto"
    ));
    assert!(flag(state, "deck.stream.variant_active@deck=a,variant=2"));
    assert!(!hand.view.flag(MENU));
}

#[kithara::test]
fn the_popover_closes_the_menu_and_the_cell_toggles_it() {
    const POP: &str = "modules/deck/transport/stream/pop";

    let mut hand = Hand::at("modules");

    hand.press(CELL);
    hand.press(POP);
    assert!(!hand.view.flag(MENU));

    hand.press(CELL);
    hand.press(CELL);
    assert!(!hand.view.flag(MENU));
}

#[kithara::test]
fn a_slot_beyond_the_ladder_reads_hidden() {
    let state = DemoReads::default();

    assert!(!flag(&state, "deck.stream.variant_hidden@deck=a,variant=2"));
    assert!(flag(&state, "deck.stream.variant_hidden@deck=a,variant=3"));
}
