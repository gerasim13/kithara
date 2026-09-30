//! A track list beside a deck that takes drops, shared by the drag tests of
//! both hosts so each proves the same session.

use std::{cell::Cell, sync::LazyLock};

use crate::{
    builtin,
    compile::{CompiledUi, compile},
    mock::TestRegistry,
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{Published, ReadValue, Reads, Scope, TableCell, TableRow, UiEvent},
    source::{MemResolver, UiConfig},
    view,
};

pub(crate) const LOAD: &str = "demo.load@deck=b";

pub(crate) const WIDTH: f32 = 400.0;
pub(crate) const HEIGHT: f32 = 200.0;

fn row(title: &'static str, url: Option<&'static str>) -> TableRow<'static> {
    let row = TableRow::new(vec![TableCell::text("title", title)], false);
    match url {
        Some(url) => row.with_drag(url),
        None => row,
    }
}

static LISTED: LazyLock<Vec<TableRow<'static>>> = LazyLock::new(|| {
    vec![
        row("One", Some("file:///one.mp3")),
        row("Two", Some("file:///two.mp3")),
        row("Three", Some("file:///three.mp3")),
    ]
});

static REORDERED: LazyLock<Vec<TableRow<'static>>> = LazyLock::new(|| {
    vec![
        row("Three", Some("file:///three.mp3")),
        row("One", Some("file:///one.mp3")),
        row("Two", Some("file:///two.mp3")),
    ]
});

static FILTERED: LazyLock<Vec<TableRow<'static>>> = LazyLock::new(|| {
    vec![
        row("One", Some("file:///one.mp3")),
        row("Three", Some("file:///three.mp3")),
    ]
});

static BARE: LazyLock<Vec<TableRow<'static>>> =
    LazyLock::new(|| vec![row("One", None), row("Two", None), row("Three", None)]);

pub(crate) const DRAGGED: &str = "file:///two.mp3";
pub(crate) const DRAGGED_TITLE: &str = "Two";
pub(crate) const FILTERED_IN_ITS_PLACE: &str = "file:///three.mp3";
pub(crate) const REORDERED_IN_ITS_PLACE: &str = "file:///one.mp3";

#[derive(Clone, Copy)]
pub(crate) enum Rows {
    Listed,
    Filtered,
    Reordered,
    Bare,
}

pub(crate) struct DropReads {
    pub(crate) rows: Cell<Rows>,
}

impl DropReads {
    pub(crate) const fn new(rows: Rows) -> Self {
        Self {
            rows: Cell::new(rows),
        }
    }
}

impl Reads for DropReads {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = Scope::split(endpoint).0;
        match id {
            "library.visible_tracks" => Some(ReadValue::Table(match self.rows.get() {
                Rows::Listed => &LISTED[..],
                Rows::Filtered => &FILTERED[..],
                Rows::Reordered => &REORDERED[..],
                Rows::Bare => &BARE[..],
            })),
            "library.browser_open" => Some(ReadValue::Bool(true)),
            _ => None,
        }
    }
}

const LISTED_IN_PLACE: &str = r#"(schema: "kithara.module", version: 1, id: "library",
    root: Column(gap: 0.0, pad: 0.0, size: (w: Fill, h: Fill), children: [
        Table(
            id: "tracks",
            size: (w: Fill, h: Fill),
            read: Model(id: "library.visible_tracks"),
            columns: [(id: "title", label: "TITLE", style: Primary, width: 180.0)],
        ),
    ]))"#;

const LISTED_ON_A_POPOVER: &str = r#"(schema: "kithara.module", version: 1, id: "library",
    root: Popover(
        id: "browser",
        open: Model(id: "library.browser_open"),
        anchor: Spacer(id: "strip", size: (w: Fill, h: Fixed(20.0))),
        content: Table(
            id: "tracks",
            size: (w: Fixed(180.0), h: Fixed(160.0)),
            read: Model(id: "library.visible_tracks"),
            columns: [(id: "title", label: "TITLE", style: Primary, width: 180.0)],
        ),
    ))"#;

pub(crate) const RESIZED: &str = "library/tracks/width/title";
pub(crate) const BORDER: (f32, f32) = (100.0, 10.0);
pub(crate) const WIDER: (f32, f32) = (130.0, 10.0);

fn two_columns(write_width: &str) -> String {
    format!(
        r#"(schema: "kithara.module", version: 1, id: "library",
    root: Column(gap: 0.0, pad: 0.0, size: (w: Fill, h: Fill), children: [
        Table(
            id: "tracks",
            size: (w: Fill, h: Fill),
            read: Model(id: "library.visible_tracks"),{write_width}
            columns: [
                (id: "title", label: "TITLE", style: Primary, width: 100.0),
                (id: "artist", label: "ARTIST", style: Secondary, width: 80.0),
            ],
        ),
    ]))"#
    )
}

pub(crate) fn compiled() -> CompiledUi {
    compiled_with(LISTED_IN_PLACE)
}

pub(crate) fn compiled_engine_hosted() -> CompiledUi {
    compiled_with(&LISTED_IN_PLACE.replacen(r#"id: "library""#, r#"id: "app-library""#, 1))
}

pub(crate) fn compiled_on_a_popover() -> CompiledUi {
    compiled_with(LISTED_ON_A_POPOVER)
}

pub(crate) fn compiled_with_columns() -> CompiledUi {
    compiled_with(&two_columns(""))
}

pub(crate) fn compiled_with_column_widths() -> CompiledUi {
    compiled_with(&two_columns(
        r#"
            write_width: Parameter(id: "demo.width"),"#,
    ))
}

fn compiled_with(library: &str) -> CompiledUi {
    let mut registry = TestRegistry::default();
    registry.insert(
        EndpointCategory::Model,
        "library.visible_tracks",
        EndpointDesc::new(ValueKind::Table),
    );
    registry.insert(
        EndpointCategory::Model,
        "library.browser_open",
        EndpointDesc::new(ValueKind::Bool),
    );
    registry.insert(
        EndpointCategory::Parameter,
        "demo.width",
        EndpointDesc::new(ValueKind::Scalar).with_scope("column"),
    );
    registry.insert(
        EndpointCategory::Command,
        "demo.load",
        EndpointDesc::new(ValueKind::Text).with_scope("deck"),
    );
    let mut resolver = MemResolver::default();
    resolver.insert(
        "fixture.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "fixture",
            root: Split(axis: Horizontal, children: [
                (node: Module(instance: "library", source: "library.kmodule.ron",
                    size: (w: Fixed(200.0), h: Fill))),
                (node: Module(instance: "deck-b", source: "deck.kmodule.ron", with: { "deck": "b" },
                    size: (w: Fill, h: Fill))),
            ]))"#,
    );
    resolver.insert("library.kmodule.ron", library);
    resolver.insert(
        "deck.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "deck",
            parameters: ["deck"],
            drop: Some((write: Command(id: "demo.load", with: { "deck": "$deck" }))),
            root: Column(gap: 0.0, pad: 0.0, size: (w: Fill, h: Fill), children: [
                Text(id: "name", style: MicroLabel, label: "DECK", size: (w: Fill, h: Fill)),
            ]))"#,
    );
    compile(
        "fixture.klayout.ron",
        &resolver,
        &registry,
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
        &view::EMPTY,
    )
    .unwrap_or_else(|error| panic!("the drop fixture must compile: {error}"))
}

pub(crate) const FROM: (f32, f32) = (60.0, 60.0);
pub(crate) const CARRY: [(f32, f32); 2] = [(64.0, 62.0), (150.0, 80.0)];
pub(crate) const OVER_THE_DECK: (f32, f32) = (300.0, 100.0);
pub(crate) const AWAY: (f32, f32) = (100.0, 150.0);
pub(crate) const ZONE: &str = "deck-b/drop";

pub(crate) fn writes(ui: &CompiledUi, published: &[Published], reads: &dyn Reads) -> Vec<UiEvent> {
    let mut view = view::ViewState::default();
    published
        .iter()
        .filter(|event| matches!(event, Published::Gesture { .. }))
        .filter_map(|event| ui.views().settle(event.clone(), reads, &mut view))
        .collect()
}

pub(crate) fn load(url: &str) -> UiEvent {
    UiEvent::Write {
        key: LOAD.to_owned(),
        value: crate::render::WriteValue::Text(url.to_owned()),
    }
}

pub(crate) trait DropHost {
    fn carry_to(&mut self, ui: &CompiledUi, reads: &DropReads, at: (f32, f32));
    fn let_go(&mut self, ui: &CompiledUi, reads: &DropReads, at: (f32, f32)) -> Vec<Published>;
    fn open(ui: &CompiledUi, reads: &DropReads) -> Self;
    fn pick_up(&mut self, ui: &CompiledUi, reads: &DropReads, at: (f32, f32));
    fn refresh(&mut self, ui: &CompiledUi, reads: &DropReads);
}

fn carried<H: DropHost>(host: &mut H, ui: &CompiledUi, reads: &DropReads, from: (f32, f32)) {
    host.pick_up(ui, reads, from);
    for at in CARRY {
        host.carry_to(ui, reads, at);
    }
    host.carry_to(ui, reads, OVER_THE_DECK);
}

fn let_go_over_the_deck<H: DropHost>(
    host: &mut H,
    ui: &CompiledUi,
    reads: &DropReads,
) -> Vec<UiEvent> {
    let published = host.let_go(ui, reads, OVER_THE_DECK);
    writes(ui, &published, reads)
}

fn dropped_from<H: DropHost>(ui: &CompiledUi, rows: Rows, from: (f32, f32)) -> Vec<UiEvent> {
    let reads = DropReads::new(rows);
    let mut host = H::open(ui, &reads);
    carried(&mut host, ui, &reads, from);
    let_go_over_the_deck(&mut host, ui, &reads)
}

fn dropped_after<H: DropHost>(rows: Rows) -> (Vec<UiEvent>, Vec<UiEvent>) {
    let ui = compiled();
    let reads = DropReads::new(Rows::Listed);
    let mut host = H::open(&ui, &reads);

    carried(&mut host, &ui, &reads, FROM);
    reads.rows.set(rows);
    host.refresh(&ui, &reads);
    let writes = let_go_over_the_deck(&mut host, &ui, &reads);
    carried(&mut host, &ui, &reads, FROM);
    (writes, let_go_over_the_deck(&mut host, &ui, &reads))
}

pub(crate) fn a_row_dropped_on_a_deck_delivers_the_decks_write_with_the_rows_drag_data<
    H: DropHost,
>() {
    let writes = dropped_from::<H>(&compiled(), Rows::Listed, FROM);

    assert_eq!(writes, [load(DRAGGED)]);
}

pub(crate) fn a_row_dropped_after_the_list_was_filtered_delivers_the_row_that_was_dragged<
    H: DropHost,
>() {
    let (writes, after) = dropped_after::<H>(Rows::Filtered);

    assert_eq!(
        after,
        [load(FILTERED_IN_ITS_PLACE)],
        "the list must show the filtered rows by the time of the drop, or the drop proves nothing"
    );
    assert_eq!(writes, [load(DRAGGED)]);
}

pub(crate) fn a_row_dropped_after_the_list_was_reordered_delivers_the_row_that_was_dragged<
    H: DropHost,
>() {
    let (writes, after) = dropped_after::<H>(Rows::Reordered);

    assert_eq!(
        after,
        [load(REORDERED_IN_ITS_PLACE)],
        "the list must show the re-ordered rows by the time of the drop, or the drop proves nothing"
    );
    assert_eq!(writes, [load(DRAGGED)]);
}

pub(crate) fn a_row_without_drag_data_delivers_nothing_when_dropped<H: DropHost>() {
    let writes = dropped_from::<H>(&compiled(), Rows::Bare, FROM);

    assert_eq!(writes, []);
}

pub(crate) fn a_row_carried_into_a_zone_and_let_go_outside_it_delivers_nothing<H: DropHost>() {
    let ui = compiled();
    let reads = DropReads::new(Rows::Listed);
    let mut host = H::open(&ui, &reads);

    carried(&mut host, &ui, &reads, FROM);
    host.carry_to(&ui, &reads, AWAY);
    let published = host.let_go(&ui, &reads, AWAY);

    assert_eq!(writes(&ui, &published, &reads), []);
}

pub(crate) fn a_row_dropped_from_a_list_on_a_popover_delivers_the_decks_write<H: DropHost>() {
    const STRIP: f32 = 20.0;
    const FROM_THE_POPOVER: (f32, f32) = (FROM.0, FROM.1 + STRIP);
    let writes = dropped_from::<H>(&compiled_on_a_popover(), Rows::Listed, FROM_THE_POPOVER);

    assert_eq!(writes, [load(DRAGGED)]);
}

macro_rules! drop_suite {
    ($host:ty) => {
        #[kithara::test]
        fn a_row_dropped_on_a_deck_delivers_the_decks_write_with_the_rows_drag_data() {
            $crate::render::drop_fixture::a_row_dropped_on_a_deck_delivers_the_decks_write_with_the_rows_drag_data::<$host>();
        }

        #[kithara::test]
        fn a_row_dropped_after_the_list_was_filtered_delivers_the_row_that_was_dragged() {
            $crate::render::drop_fixture::a_row_dropped_after_the_list_was_filtered_delivers_the_row_that_was_dragged::<$host>();
        }

        #[kithara::test]
        fn a_row_dropped_after_the_list_was_reordered_delivers_the_row_that_was_dragged() {
            $crate::render::drop_fixture::a_row_dropped_after_the_list_was_reordered_delivers_the_row_that_was_dragged::<$host>();
        }

        #[kithara::test]
        fn a_row_without_drag_data_delivers_nothing_when_dropped() {
            $crate::render::drop_fixture::a_row_without_drag_data_delivers_nothing_when_dropped::<
                $host,
            >();
        }

        #[kithara::test]
        fn a_row_carried_into_a_zone_and_let_go_outside_it_delivers_nothing() {
            $crate::render::drop_fixture::a_row_carried_into_a_zone_and_let_go_outside_it_delivers_nothing::<$host>();
        }

        #[kithara::test]
        fn a_row_dropped_from_a_list_on_a_popover_delivers_the_decks_write() {
            $crate::render::drop_fixture::a_row_dropped_from_a_list_on_a_popover_delivers_the_decks_write::<$host>();
        }
    };
}

pub(crate) use drop_suite;
