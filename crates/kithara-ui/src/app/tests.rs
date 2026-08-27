use std::{
    env,
    fs::{File, create_dir_all},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use kithara_platform::time::Duration;
use kithara_test_utils::kithara;
use masonry::{core::CursorIcon, vello::Scene};

use super::{App, Config, RunError, Ui, scenario::Scenario};
use crate::{
    builtin,
    draw::{Pt, Rect, Rgba},
    ids::{DocId, EndpointId, SourceUri},
    interact::{Input, Key, MOUSE, Modifiers, PointerInput, PointerPhase},
    registry::{EndpointCategory, EndpointDesc, EndpointRegistry, ValueKind},
    render::{
        ControlAction, ReadValue, Reads, Skin, StereoLevels, UiEvent,
        document::{Clock, Ctx},
    },
    shaping::FontPolicy,
    source::MemResolver,
};

/// A registry that answers for the one endpoint the fixture binds to.
struct Registry(&'static str, EndpointDesc);

impl EndpointRegistry for Registry {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        (category == EndpointCategory::Model && id.0 == self.0).then_some(&self.1)
    }
}

/// An application that shows one of two documents and swaps between them
/// whenever the one it is showing publishes an activation.
#[derive(Default)]
struct Swapper {
    lit: bool,
}

impl Reads for Swapper {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.lit").then_some(ReadValue::Bool(self.lit))
    }
}

impl App for Swapper {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        if self.lit {
            "lit.klayout.ron"
        } else {
            "dim.klayout.ron"
        }
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && action == ControlAction::Activate
        {
            self.lit = !self.lit;
        }
    }
}

/// An application that wears one of two skins and turns to the other whenever
/// the document it is showing publishes an activation. Which skin a player
/// wears is the player's to decide, so switching one at runtime is this and
/// nothing more.
struct Dresser<'a> {
    lit: bool,
    off: &'a Skin,
    on: &'a Skin,
}

impl<'a> Dresser<'a> {
    const fn wearing(off: &'a Skin, on: &'a Skin) -> Self {
        Self {
            lit: false,
            off,
            on,
        }
    }
}

impl Reads for Dresser<'_> {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.lit").then_some(ReadValue::Bool(self.lit))
    }
}

impl App for Dresser<'_> {
    /// The one document it shows either way: what changes here is the skin.
    fn document(&self) -> &str {
        "dim.klayout.ron"
    }

    fn skin(&self) -> &Skin {
        if self.lit { self.on } else { self.off }
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && action == ControlAction::Activate
        {
            self.lit = !self.lit;
        }
    }
}

/// An application holding one value, which its one control writes. Meters read
/// the same value as a stereo level, which is the shape their plan asks for.
struct Dial {
    stereo: bool,
    value: f64,
}

impl Dial {
    fn new(stereo: bool) -> Self {
        Self { stereo, value: 0.5 }
    }
}

impl Reads for Dial {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        if id != "fixture.dial" {
            return None;
        }
        Some(if self.stereo {
            ReadValue::Stereo(StereoLevels {
                l: 0.4,
                r: 0.4,
                volume: num_traits::cast::AsPrimitive::<f32>::as_(self.value),
            })
        } else {
            ReadValue::Scalar(self.value)
        })
    }
}

impl App for Dial {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "one.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && let ControlAction::SetScalar(value) = action
        {
            self.value = value;
        }
    }
}

struct TickingDial {
    value: f64,
}

impl Reads for TickingDial {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.dial").then_some(ReadValue::Scalar(self.value))
    }
}

impl App for TickingDial {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "one.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn tick(&mut self) {
        self.value = 0.75;
    }

    fn update(&mut self, _event: UiEvent) {}
}

#[derive(Default)]
struct Typed {
    query: String,
}

impl Reads for Typed {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.tree").then_some(ReadValue::Tree(&[]))
    }
}

impl App for Typed {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "one.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::LibraryQuery(query) = event {
            self.query = query;
        }
    }
}

struct Board;

impl Reads for Board {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.flag").then_some(ReadValue::Bool(false))
    }
}

impl App for Board {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "board.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, _event: UiEvent) {}
}

struct InteractionRegistry {
    dial: EndpointDesc,
    flag: EndpointDesc,
}

impl Default for InteractionRegistry {
    fn default() -> Self {
        Self {
            dial: EndpointDesc::new(ValueKind::Scalar),
            flag: EndpointDesc::new(ValueKind::Bool),
        }
    }
}

impl EndpointRegistry for InteractionRegistry {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        if category != EndpointCategory::Model {
            return None;
        }
        match id.0.as_str() {
            "fixture.dial" => Some(&self.dial),
            "fixture.flag" => Some(&self.flag),
            _ => None,
        }
    }
}

struct InteractionBoard {
    active: bool,
    value: f64,
}

impl Reads for InteractionBoard {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        match id {
            "fixture.dial" => Some(ReadValue::Scalar(self.value)),
            "fixture.flag" => Some(ReadValue::Bool(self.active)),
            _ => None,
        }
    }
}

impl App for InteractionBoard {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "interactions.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        let UiEvent::Control { path, action } = event else {
            return;
        };
        match action {
            ControlAction::Activate if path == "demo/fire" => self.active = true,
            ControlAction::SetScalar(value) if path == "demo/dial" => self.value = value,
            _ => {}
        }
    }
}

/// A document holding one control, so a gesture aimed at the middle of the
/// window lands on it whatever the control turns out to be.
///
/// The module is named `gallery-knobs` because that name is on the list of
/// modules whose contents an engine drives from above. A module off that list
/// gives its controls their own input, which is the easier half of the
/// contract; the harder half is the one the real documents use.
fn one_control(control: &str) -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "one.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "one",
            root: Module(instance: "demo", source: "one.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "one.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "gallery-knobs", chrome: Plain,
                root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [{control}]))"#
        ),
    );
    resolver
}

fn focused_search<'a>(endpoints: &'a Registry, resolver: &'a MemResolver) -> Ui<'a, Typed> {
    let config = Config::builder()
        .endpoints(endpoints)
        .resolver(resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Typed::default(), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the search box must mount: {error}"));
    ui.frame(Duration::from_millis(16));
    ui.scene()
        .unwrap_or_else(|error| panic!("the search box must draw: {error}"));
    let at = Pt {
        x: skin().tree.search_icon_width + 10.0,
        y: skin().tree.search_height / 2.0,
    };
    ui.input(press(at, PointerPhase::Move));
    ui.input(press(at, PointerPhase::Down));
    ui.input(press(at, PointerPhase::Up));
    ui
}

fn tree_fixture() -> (Registry, MemResolver) {
    (
        Registry("fixture.tree", EndpointDesc::new(ValueKind::Tree)),
        one_control(
            r#"Tree(id: "control", size: (w: Fill, h: Fill), read: Model(id: "fixture.tree"))"#,
        ),
    )
}

fn board() -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "board.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "board",
            root: Module(instance: "demo", source: "board.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "board.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "gallery-knobs", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                Chip(id: "one", size: Some((w: Fixed(80.0), h: Fixed(40.0))), label: "ONE", read: Model(id: "fixture.flag")),
                Chip(id: "two", size: Some((w: Fixed(80.0), h: Fixed(40.0))), label: "TWO", read: Model(id: "fixture.flag")),
                Chip(id: "three", size: Some((w: Fixed(80.0), h: Fixed(40.0))), label: "THREE", read: Model(id: "fixture.flag")),
            ]))"#,
    );
    resolver
}

fn interaction_board() -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "interactions.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "interactions",
            root: Module(instance: "demo", source: "interactions.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "interactions.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "gallery-knobs", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 12.0, pad: 12.0, children: [
                Chip(id: "fire", size: Some((w: Fixed(80.0), h: Fixed(40.0))), label: "FIRE", read: Model(id: "fixture.flag")),
                Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial")),
            ]))"#,
    );
    resolver
}

/// What one drag produced: the value the application read after each step, and
/// what the control actually drew at that moment.
struct Dragged {
    drawn: Vec<u64>,
    seen: Vec<f64>,
}

/// Drags one control across the window, step by step.
fn drag(control: &Draggable) -> Dragged {
    const STEPS: i16 = 4;

    let (control, from, to) = (control.control, control.from, control.to);
    let stereo = control.starts_with("Vu");
    let kind = if stereo {
        ValueKind::Stereo
    } else {
        ValueKind::Scalar
    };
    let endpoints = Registry("fixture.dial", EndpointDesc::new(kind));
    let resolver = one_control(control);
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(stereo), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("{control} must mount: {error}"));
    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("{control} must draw: {error}"));

    ui.input(press(from, PointerPhase::Move));
    ui.input(press(from, PointerPhase::Down));
    let mut drawn = Vec::new();
    let mut seen = Vec::new();
    for step in 1..=STEPS {
        let fraction = f32::from(step) / f32::from(STEPS);
        let at = Pt {
            x: from.x + (to.x - from.x) * fraction,
            y: from.y + (to.y - from.y) * fraction,
        };
        ui.input(press(at, PointerPhase::Move));
        seen.push(ui.app().value);
        let frame = ui
            .render()
            .unwrap_or_else(|error| panic!("{control} must draw mid-drag: {error}"));
        drawn.push(geometry(frame.scene()));
    }
    ui.input(press(to, PointerPhase::Up));
    Dragged { drawn, seen }
}

/// Counts every source the compiler read, so a test can see whether the
/// document was compiled again at all.
struct Counted<'a> {
    inner: &'a dyn crate::source::SourceResolver,
    loads: std::cell::Cell<usize>,
}

impl crate::source::SourceResolver for Counted<'_> {
    fn load(
        &self,
        base: Option<&SourceUri>,
        rel: &str,
    ) -> Result<crate::source::LoadedSource, crate::error::UiDocError> {
        self.loads.set(self.loads.get() + 1);
        self.inner.load(base, rel)
    }
}

/// The wheel is how a knob is nudged without dragging it, how the hero wave
/// zooms and how a list scrolls. None of it works unless the window forwards
/// the notch and this layer carries it through.
#[kithara::test]
fn a_wheel_notch_over_a_knob_steps_it() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let mut scenario = Scenario::mount(
        Dial::new(false),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    );
    let before = scenario.app().value;

    // A wheel away from the hand reads as a negative delta, and that is the
    // direction that raises a control.
    scenario.wheel("demo/dial", -1.0);

    assert!(
        scenario.app().value > before,
        "a notch over the knob must raise it, but it stayed at {before}"
    );
}

#[kithara::test]
fn a_typed_character_reaches_a_focused_retained_text_field() {
    let (endpoints, resolver) = tree_fixture();
    let mut ui = focused_search(&endpoints, &resolver);

    ui.input(Input::KeyPressed {
        key: Key::Character("a"),
        modifiers: Modifiers::default(),
        text: Some("a"),
    });

    assert_eq!(ui.app().query, "a");
}

#[kithara::test]
fn a_modifier_change_alone_does_not_edit_the_focused_field() {
    let (endpoints, resolver) = tree_fixture();
    let mut ui = focused_search(&endpoints, &resolver);
    ui.input(Input::ModifiersChanged(Modifiers::new(
        false, false, false, true,
    )));

    assert!(ui.app().query.is_empty());
}

#[kithara::test]
fn a_key_release_does_not_repeat_text_input() {
    let (endpoints, resolver) = tree_fixture();
    let mut ui = focused_search(&endpoints, &resolver);
    ui.input(Input::KeyPressed {
        key: Key::Character("a"),
        modifiers: Modifiers::default(),
        text: Some("a"),
    });
    ui.input(Input::KeyReleased {
        key: Key::Character("a"),
        modifiers: Modifiers::default(),
    });

    assert_eq!(ui.app().query, "a");
}

#[kithara::test]
fn controls_on_one_page_publish_only_their_own_activation() {
    let endpoints = Registry("fixture.flag", EndpointDesc::new(ValueKind::Bool));
    let resolver = board();
    let mut scenario = Scenario::mount(
        Board,
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    );

    for path in ["demo/one", "demo/two", "demo/three"] {
        let mark = scenario.published().len();
        scenario.click(path);

        assert_eq!(
            &scenario.published()[mark..],
            [UiEvent::Control {
                path: path.to_owned(),
                action: ControlAction::Activate,
            }]
        );
    }
}

#[kithara::test]
fn named_press_drag_and_wheel_publish_events_and_leave_a_picture() {
    const SIZE: (u32, u32) = (240, 120);

    let endpoints = InteractionRegistry::default();
    let resolver = interaction_board();
    let mut scenario = Scenario::mount(
        InteractionBoard {
            active: false,
            value: 0.25,
        },
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        SIZE,
        1.0,
    );
    let capture = scenario_capture_dir();

    let mark = scenario.published().len();
    scenario.press("demo/fire");
    assert_eq!(
        &scenario.published()[mark..],
        [UiEvent::Control {
            path: "demo/fire".to_owned(),
            action: ControlAction::Activate,
        }]
    );
    photograph_scenario(&mut scenario, capture.as_deref(), "01-press", SIZE);
    let mark = scenario.published().len();
    scenario.release("demo/fire");
    assert_eq!(scenario.published().len(), mark);

    let mark = scenario.published().len();
    scenario.drag(
        "demo/dial",
        Pt { x: 0.5, y: 1.0 },
        Pt { x: 0.5, y: 0.25 },
        4,
    );
    let drag = &scenario.published()[mark..];
    let values = drag
        .iter()
        .filter_map(|event| match event {
            UiEvent::Control {
                path,
                action: ControlAction::SetScalar(value),
            } if path == "demo/dial" => Some(*value),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(values.len() > 1, "the named drag must publish every step");
    assert!(
        values.windows(2).all(|pair| pair[1] > pair[0]),
        "the named upward drag published {values:?}"
    );
    photograph_scenario(&mut scenario, capture.as_deref(), "02-drag", SIZE);

    let before = scenario.app().value;
    let mark = scenario.published().len();
    scenario.wheel("demo/dial", -1.0);
    let [
        UiEvent::Control {
            path,
            action: ControlAction::SetScalar(value),
        },
    ] = &scenario.published()[mark..]
    else {
        panic!("the named wheel must publish one scalar event")
    };
    assert_eq!(path, "demo/dial");
    assert_ne!(*value, before, "the named wheel must move the dial");
    assert_eq!(scenario.app().value, *value);
    photograph_scenario(&mut scenario, capture.as_deref(), "03-wheel", SIZE);
}

/// The page behind a document is the skin's, and a host clears its target to it
/// before the scene lands. The retained window's first answer was black, which
/// showed through wherever a document left its rectangle bare and read as a
/// difference between the hosts.
#[kithara::test]
fn a_mounted_ui_takes_its_page_colour_from_the_skin_document() {
    let endpoints = Registry("fixture.lit", EndpointDesc::new(ValueKind::Bool));
    let resolver = resolver();
    let blue = page_skin("fixture-blue", "#123456");
    let ui = Ui::new(
        Dresser::wearing(&blue, skin()),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the fixture must mount: {error}"));

    assert_eq!(ui.background(), BLUE_PAGE);
}

/// A player turns to another skin while it is running, which is the whole point
/// of a skin being a document. The page colour is the cheapest thing to read
/// back, and it is the host's own answer rather than anything the document
/// painted.
#[kithara::test]
fn a_running_ui_follows_its_application_to_another_skin() {
    let endpoints = Registry("fixture.lit", EndpointDesc::new(ValueKind::Bool));
    let resolver = resolver();
    let blue = page_skin("fixture-blue", "#123456");
    let mut scenario = Scenario::mount(
        Dresser::wearing(skin(), &blue),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    );
    assert_ne!(
        scenario.background(),
        BLUE_PAGE,
        "the fixture must open in the skin it was mounted in"
    );

    scenario.click("demo/swap");

    assert_eq!(scenario.background(), BLUE_PAGE);
}

/// And the document is compiled again for it: a skin settles the room every
/// control needs, so a tree built against the old one is the wrong shape.
#[kithara::test]
fn turning_to_another_skin_compiles_the_document_again() {
    let endpoints = Registry("fixture.lit", EndpointDesc::new(ValueKind::Bool));
    let inner = resolver();
    let resolver = Counted {
        inner: &inner,
        loads: std::cell::Cell::new(0),
    };
    let blue = page_skin("fixture-blue", "#123456");
    let mut scenario = Scenario::mount(
        Dresser::wearing(skin(), &blue),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    );
    let mounted = resolver.loads.get();

    scenario.click("demo/swap");

    assert!(
        resolver.loads.get() > mounted,
        "turning to another skin must read the document again"
    );
}

/// A hand on a control publishes an action for every step it moves. Compiling
/// the page again for each of them costs more than drawing it — and the page
/// cannot have changed, because the application is still showing the same one.
#[kithara::test]
fn moving_a_control_does_not_compile_the_document_again() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let inner = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let resolver = Counted {
        inner: &inner,
        loads: std::cell::Cell::new(0),
    };
    let mut ui = Ui::new(
        Dial::new(false),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the knob must mount: {error}"));
    let mounted = resolver.loads.get();
    assert!(mounted > 0, "mounting must have compiled the document once");

    let at = |y: f32| Pt { x: 19.0, y };
    ui.input(press(at(60.0), PointerPhase::Move));
    ui.input(press(at(60.0), PointerPhase::Down));
    for step in 1_u8..=8 {
        ui.input(press(at(60.0 - f32::from(step) * 2.5), PointerPhase::Move));
    }
    ui.input(press(at(40.0), PointerPhase::Up));

    assert!(ui.app().value > 0.5, "the drag must have moved the knob");
    assert_eq!(
        resolver.loads.get(),
        mounted,
        "the document was compiled again while the hand was on the knob"
    );
}

/// A fingerprint of the geometry one frame drew, so two frames can be compared
/// without rasterising them.
fn geometry(scene: &Scene) -> u64 {
    let encoding = scene.encoding();
    encoding
        .path_data
        .iter()
        .chain(&encoding.draw_data)
        .fold(0_u64, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u64::from(*byte))
        })
}

fn scenario_capture_dir() -> Option<PathBuf> {
    let dir = env::var_os("KITHARA_UI_SCENARIO_CAPTURE").map(PathBuf::from)?;
    create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("create scenario capture {}: {error}", dir.display()));
    Some(dir)
}

fn photograph_scenario(
    scenario: &mut Scenario<'_, InteractionBoard>,
    dir: Option<&Path>,
    name: &str,
    size: (u32, u32),
) {
    let Some(dir) = dir else {
        return;
    };
    let background = scenario.background().into();
    let scene = scenario.scene();
    let rgba = crate::backends::conformance::rasterise_at(&scene, size, background)
        .unwrap_or_else(|error| panic!("rasterise scenario {name}: {error}"));
    let path = dir.join(format!("{name}.png"));
    write_png(&path, &rgba, size).unwrap_or_else(|error| panic!("write scenario {name}: {error}"));
}

fn write_png(path: &Path, rgba: &[u8], size: (u32, u32)) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), size.0, size.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(rgba))
        .map_err(|error| format!("encode {}: {error}", path.display()))
}

fn resolver() -> MemResolver {
    let mut resolver = MemResolver::default();
    for name in ["lit", "dim"] {
        resolver.insert(
            &format!("{name}.klayout.ron"),
            &format!(
                r#"(schema: "kithara.layout", version: 1, id: "{name}",
                    root: Module(
                        instance: "demo",
                        source: "{name}.kmodule.ron",
                        size: (w: Fill, h: Fill),
                    ))"#
            ),
        );
        resolver.insert(
            &format!("{name}.kmodule.ron"),
            &format!(
                r#"(schema: "kithara.module", version: 1, id: "{name}", chrome: Plain,
                    root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                        Chip(
                            id: "swap",
                            size: Some((w: Fixed(120.0), h: Fixed(40.0))),
                            label: "SWAP",
                            read: Model(id: "fixture.lit"),
                        ),
                    ]))"#
            ),
        );
    }
    resolver
}

/// The skin every fixture wears unless it is the skin itself under test.
/// Shared rather than resolved per test: resolving one embeds the fonts, and
/// the suite mounts hundreds of documents.
fn skin() -> &'static Skin {
    static SKIN: LazyLock<Skin> = LazyLock::new(|| {
        Skin::resolve_with_font_policy(
            builtin::skin_doc().clone(),
            builtin::text_doc(),
            &SourceUri("fixture:app-input".to_owned()),
            FontPolicy::Embedded,
        )
        .unwrap_or_else(|error| panic!("the fixture skin must resolve: {error}"))
    });
    &SKIN
}

/// The page colour `page_skin` is asked for below, read back the way a host
/// reads it.
const BLUE_PAGE: Rgba = Rgba {
    a: 1.0,
    b: 0x56 as f32 / 255.0,
    g: 0x34 as f32 / 255.0,
    r: 0x12 as f32 / 255.0,
};

/// A skin of its own, told apart from the fixture one by the page colour it
/// names and by the identity a host follows it on.
fn page_skin(id: &str, bg: &str) -> Skin {
    let mut doc = builtin::skin_doc().clone();
    doc.id = DocId(id.to_owned());
    doc.palette.bg = bg.to_owned();
    Skin::resolve_with_font_policy(
        doc,
        builtin::text_doc(),
        &SourceUri("fixture:app-input".to_owned()),
        FontPolicy::Embedded,
    )
    .unwrap_or_else(|error| panic!("the fixture skin must resolve: {error}"))
}

fn press(at: Pt, phase: PointerPhase) -> Input<'static> {
    Input::Pointer(PointerInput::new(MOUSE, None, phase, Some(at), 1))
}

/// The cursor under the pointer reaches the runner that owns the window.
///
/// The retained host answers the question once, when the pointer moves, and
/// leaves the answer queued. A runner that never collects it shows one cursor
/// for the life of the window however carefully the tree resolves the shape.
#[kithara::test]
fn a_hover_hands_the_runner_the_cursor_under_the_pointer() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let mut ui = Ui::new(
        Dial::new(false),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the knob must mount: {error}"));
    ui.take_cursor();

    ui.input(press(Pt { x: 19.0, y: 60.0 }, PointerPhase::Move));

    assert_eq!(
        ui.take_cursor(),
        Some(CursorIcon::NsResize),
        "a hover over the knob must hand the runner the shape the knob asks for"
    );
}

/// An application whose one menu opens and closes on activation, the way the
/// burger of the shipped app bar does, and which remembers whether a press ever
/// reached the control the menu keeps inside itself.
#[derive(Default)]
struct Menu {
    open: bool,
    picked: bool,
}

impl Reads for Menu {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.menu").then_some(ReadValue::Bool(self.open))
    }
}

impl App for Menu {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "one.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        let UiEvent::Control { path, action } = event else {
            return;
        };
        if action != ControlAction::Activate {
            return;
        }
        if path.ends_with("inside") {
            self.picked = true;
        } else {
            self.open = !self.open;
        }
    }
}

/// The endpoints the menu fixture binds to: the flag the document reads to know
/// whether the menu stands open, and the commands its two pressables publish.
struct MenuEndpoints {
    open: EndpointDesc,
    press: EndpointDesc,
}

impl Default for MenuEndpoints {
    fn default() -> Self {
        Self {
            open: EndpointDesc::new(ValueKind::Bool),
            press: EndpointDesc::new(ValueKind::Trigger),
        }
    }
}

impl EndpointRegistry for MenuEndpoints {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        match (category, id.0.as_str()) {
            (EndpointCategory::Model, "fixture.menu") => Some(&self.open),
            (EndpointCategory::Command, "fixture.toggle" | "fixture.pick") => Some(&self.press),
            _ => None,
        }
    }
}

/// A burger with a menu hanging on it, the menu holding a control of its own.
const MENU: &str = r#"Popover(id: "menu", open: Model(id: "fixture.menu"), align: Start,
    anchor: Pressable(id: "burger", press: Command(id: "fixture.toggle"),
        child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(20.0))))),
    content: Pressable(id: "inside", press: Command(id: "fixture.pick"),
        child: Spacer(id: "content", size: Some((w: Fixed(100.0), h: Fixed(60.0))))))"#;

/// The same burger with no menu on it, which is the picture a shut menu owes.
const NO_MENU: &str = r#"Pressable(id: "burger", press: Command(id: "fixture.toggle"),
    child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(20.0)))))"#;

/// Mounts one of the menu documents and hands it to the check.
fn with_document(control: &str, check: impl FnOnce(Ui<'_, Menu>)) {
    let endpoints = MenuEndpoints::default();
    let resolver = one_control(control);
    let ui = Ui::new(
        Menu::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the menu fixture must mount: {error}"));
    check(ui);
}

/// Puts one whole press through the hand at one point.
fn press_at<A: App>(ui: &mut Ui<'_, A>, at: Pt) {
    ui.input(press(at, PointerPhase::Move));
    ui.input(press(at, PointerPhase::Down));
    ui.input(press(at, PointerPhase::Up));
}

/// Presses the anchor the menu hangs on, wherever the layout put it.
fn press_the_anchor(ui: &mut Ui<'_, Menu>) {
    let anchor = ui
        .rect_of("demo/anchor")
        .unwrap_or_else(|| panic!("the anchor the menu hangs on must be laid out"));
    press_at(
        ui,
        Pt {
            x: anchor.x + anchor.w / 2.0,
            y: anchor.y + anchor.h / 2.0,
        },
    );
}

/// How much of a picture the host draws, read the way the control census in the
/// retained host reads a picture.
fn drawn_shapes<A: App>(ui: &mut Ui<'_, A>) -> u32 {
    ui.scene()
        .unwrap_or_else(|error| panic!("the menu fixture must draw: {error}"))
        .encoding()
        .n_paths
}

/// What the host draws for the menu fixture at each of the three moments the
/// document passes through: shut, opened by a press on its anchor, and shut
/// again by a press away from it.
/// A bar carrying the menu, mounted under one instance name.
fn menu_bar(instance: &str, band: &str) -> String {
    format!(
        r#"({band} node: Module(instance: "{instance}", source: "one.kmodule.ron",
            size: (w: Fill, h: Fixed(42.0))))"#
    )
}

/// A document that hangs the one menu on `bars` anchors and shows whichever bar
/// the room reaches, the way the shipped app bar carries a wide strip and a
/// narrow one and stands the other aside.
///
/// Every bar reads the one flag, so a press on the standing burger opens them
/// all as far as the document is concerned.
fn banded_bars(bars: &[String]) -> MemResolver {
    let mut resolver = MemResolver::default();
    let mut cells = bars.join(",\n                ");
    cells.push_str(
        r#",
                (weight: 1.0, node: Module(instance: "body", source: "body.kmodule.ron",
                    size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "one.klayout.ron",
        &format!(
            r#"(schema: "kithara.layout", version: 1, id: "one",
                root: Split(axis: Vertical, measure: Height, size: (w: Fill, h: Fill),
                    children: [{cells}]))"#
        ),
    );
    resolver.insert(
        "one.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "gallery-knobs", chrome: Plain,
                root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [{MENU}]))"#
        ),
    );
    resolver.insert(
        "body.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "body", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: []))"#,
    );
    resolver
}

/// Mounts a banded document in a window short enough that only the narrow bar
/// stands, presses the burger that is in the picture, and hands the result over.
fn with_bars(bars: &[String], check: impl FnOnce(Ui<'_, Menu>)) {
    let endpoints = MenuEndpoints::default();
    let resolver = banded_bars(bars);
    let mut ui = Ui::new(
        Menu::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (400, 400),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the banded fixture must mount: {error}"));
    let anchor = ui
        .rect_of("narrow/anchor")
        .unwrap_or_else(|| panic!("the bar the room reaches must be laid out"));
    press_at(
        &mut ui,
        Pt {
            x: anchor.x + anchor.w / 2.0,
            y: anchor.y + anchor.h / 2.0,
        },
    );
    assert!(
        ui.app().open,
        "the press on the standing burger must reach the application"
    );
    check(ui);
}

/// The narrow bar, which is the one a 400-tall window reaches.
fn narrow_bar() -> String {
    menu_bar("narrow", "until: Some(493.0),")
}

/// The wide bar, which the same window stands aside.
fn wide_bar() -> String {
    menu_bar("wide", "from: 493.0,")
}

#[kithara::test]
fn a_menu_whose_anchor_stands_aside_stays_out_of_the_picture() {
    let mut both = 0;
    let mut alone = 0;
    with_bars(&[narrow_bar(), wide_bar()], |mut ui| {
        both = drawn_shapes(&mut ui);
    });
    with_bars(&[narrow_bar()], |mut ui| alone = drawn_shapes(&mut ui));

    assert_eq!(
        both, alone,
        "a document carrying a second bar the room never reached drew {both} shapes where the \
         same document without it drew {alone}: the menu hanging on the anchor that stands aside \
         opened as well"
    );
}

#[kithara::test]
fn a_menu_whose_anchor_stands_aside_takes_no_box() {
    with_bars(&[narrow_bar(), wide_bar()], |ui| {
        assert_eq!(
            ui.rect_of("wide/content").map(|rect| (rect.w, rect.h)),
            Some((0.0, 0.0)),
            "the menu hanging on the bar the room never reached took a box of its own"
        );
    });
}

#[kithara::test]
fn the_menu_whose_anchor_stands_hangs_below_it() {
    with_bars(&[narrow_bar(), wide_bar()], |ui| {
        let anchor = ui
            .rect_of("narrow/anchor")
            .unwrap_or_else(|| panic!("the standing anchor must keep its box"));
        let content = ui
            .rect_of("narrow/content")
            .unwrap_or_else(|| panic!("the standing menu must take a box"));
        assert!(
            content.y >= anchor.y + anchor.h,
            "the open menu stands at y={} over the anchor it hangs on, which ends at y={}",
            content.y,
            anchor.y + anchor.h
        );
    });
}

fn menu_pictures() -> [u32; 3] {
    let mut drawn = [0; 3];
    with_document(MENU, |mut ui| {
        drawn[0] = drawn_shapes(&mut ui);

        press_the_anchor(&mut ui);
        assert!(
            ui.app().open,
            "the press on the anchor must reach the application"
        );
        drawn[1] = drawn_shapes(&mut ui);

        press_at(&mut ui, Pt { x: 200.0, y: 100.0 });
        assert!(
            !ui.app().open,
            "a press away from an open menu must reach the application as a dismissal"
        );
        drawn[2] = drawn_shapes(&mut ui);
    });
    drawn
}

#[kithara::test]
fn a_menu_the_document_holds_shut_draws_nothing_of_its_own() {
    let mut shut = 0;
    let mut bare = 0;
    with_document(MENU, |mut ui| shut = drawn_shapes(&mut ui));
    with_document(NO_MENU, |mut ui| bare = drawn_shapes(&mut ui));

    assert_eq!(
        shut, bare,
        "a menu the document holds shut must leave the same picture as no menu at all"
    );
}

#[kithara::test]
fn opening_a_menu_puts_its_surface_in_the_picture() {
    let [shut, open, _] = menu_pictures();

    assert!(
        open > shut,
        "the application opened the menu, so the host it is embedded in must draw the surface it \
         mounted for it: shut drew {shut} shapes, open drew {open}"
    );
}

#[kithara::test]
fn closing_a_menu_takes_its_surface_out_of_the_picture_again() {
    let [shut, _, shut_again] = menu_pictures();

    assert_eq!(
        shut_again, shut,
        "a menu the application closed must leave the picture exactly as it found it"
    );
}

#[kithara::test]
fn a_shut_menu_takes_no_press_over_the_room_its_surface_filled() {
    with_document(MENU, |mut ui| {
        press_the_anchor(&mut ui);
        let surface = ui
            .rect_of("demo/content")
            .unwrap_or_else(|| panic!("an open menu must lay its content out"));
        let anchor = ui
            .rect_of("demo/anchor")
            .unwrap_or_else(|| panic!("the anchor the menu hangs on must be laid out"));
        let at = Pt {
            x: surface.x + surface.w / 2.0,
            y: surface.y + surface.h / 2.0,
        };
        assert!(
            at.x < anchor.x
                || at.x >= anchor.x + anchor.w
                || at.y < anchor.y
                || at.y >= anchor.y + anchor.h,
            "the fixture must offer a point the menu covers and the burger does not: \
             surface {surface:?}, anchor {anchor:?}"
        );
        press_at(&mut ui, Pt { x: 200.0, y: 100.0 });
        assert!(
            !ui.app().open,
            "a press away from an open menu must shut it again"
        );

        press_at(&mut ui, at);

        assert!(
            !ui.app().picked,
            "a menu the document holds shut must take no press over the room its surface filled \
             while it was open"
        );
    });
}

/// An application that picks a track up when its one control is pressed and
/// carries it until it is pressed again, the way a playlist row starts a drag.
#[derive(Default)]
struct Carry {
    carrying: bool,
}

impl Reads for Carry {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.carried" && self.carrying).then_some(ReadValue::Text("Signal Path"))
    }
}

impl App for Carry {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "carry.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && action == ControlAction::Activate
        {
            self.carrying = !self.carrying;
        }
    }
}

/// The endpoints the carrying fixture binds to: what the pointer carries, and
/// the command the control it is picked up from publishes.
struct CarryEndpoints {
    carried: EndpointDesc,
    grab: EndpointDesc,
}

impl Default for CarryEndpoints {
    fn default() -> Self {
        Self {
            carried: EndpointDesc::new(ValueKind::Text),
            grab: EndpointDesc::new(ValueKind::Trigger),
        }
    }
}

impl EndpointRegistry for CarryEndpoints {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        match (category, id.0.as_str()) {
            (EndpointCategory::Model, "fixture.carried") => Some(&self.carried),
            (EndpointCategory::Command, "fixture.grab") => Some(&self.grab),
            _ => None,
        }
    }
}

/// A window that names what the pointer carries, over one control to pick a
/// track up from.
fn carry_document() -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "carry.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "carry", resize_edges: true,
            dragged: Some(Model(id: "fixture.carried")),
            root: Module(instance: "demo", source: "carry.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "carry.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "gallery-knobs", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                Pressable(id: "grab", press: Command(id: "fixture.grab"),
                    child: Spacer(id: "row", size: Some((w: Fixed(40.0), h: Fixed(20.0))))),
            ]))"#,
    );
    resolver
}

/// Mounts the carrying fixture and hands it to the check.
fn with_carry(check: impl FnOnce(Ui<'_, Carry>)) {
    let endpoints = CarryEndpoints::default();
    let resolver = carry_document();
    let ui = Ui::new(
        Carry::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the carrying fixture must mount: {error}"));
    check(ui);
}

/// What the host draws at each of the three moments the pointer passes through:
/// empty, carrying a track, and empty again. The pointer stands in the same
/// place throughout, so the load is the only thing that differs.
fn carrying_pictures() -> [u32; 3] {
    let mut drawn = [0; 3];
    with_carry(|mut ui| {
        let over = Pt { x: 160.0, y: 90.0 };
        ui.input(press(over, PointerPhase::Move));
        drawn[0] = drawn_shapes(&mut ui);

        let grab = ui
            .rect_of("demo/row")
            .unwrap_or_else(|| panic!("the control a track is picked up from must be laid out"));
        let grab = Pt {
            x: grab.x + grab.w / 2.0,
            y: grab.y + grab.h / 2.0,
        };
        press_at(&mut ui, grab);
        assert!(
            ui.app().carrying,
            "the press must reach the application and pick the track up"
        );
        ui.input(press(over, PointerPhase::Move));
        drawn[1] = drawn_shapes(&mut ui);

        press_at(&mut ui, grab);
        assert!(
            !ui.app().carrying,
            "the second press must reach the application and put the track down"
        );
        ui.input(press(over, PointerPhase::Move));
        drawn[2] = drawn_shapes(&mut ui);
    });
    drawn
}

/// Presents and completes one frame, so what is read after it is about the one
/// thing the test does next.
fn settle_frame(ui: &mut Ui<'_, Carry>) {
    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("the carrying fixture must draw: {error}"));
    let _ = ui.complete_frame();
}

/// Whether moving the pointer asks the host for another frame, with a track
/// picked up first or with nothing carried at all.
fn frame_asked_while_moving(carrying: bool) -> bool {
    let mut asked = false;
    with_carry(|mut ui| {
        if carrying {
            let grab = ui.rect_of("demo/row").unwrap_or_else(|| {
                panic!("the control a track is picked up from must be laid out")
            });
            press_at(
                &mut ui,
                Pt {
                    x: grab.x + grab.w / 2.0,
                    y: grab.y + grab.h / 2.0,
                },
            );
            assert!(
                ui.app().carrying,
                "the press must reach the application and pick the track up"
            );
        }
        settle_frame(&mut ui);

        ui.input(press(Pt { x: 160.0, y: 90.0 }, PointerPhase::Move));

        asked = ui.needs_frame();
    });
    asked
}

#[kithara::test]
fn a_carried_track_asks_for_a_frame_as_the_pointer_moves() {
    assert!(
        frame_asked_while_moving(true),
        "a track is drawn under the pointer, so it has to follow it: a window carrying one must \
         paint again as the pointer moves"
    );
}

#[kithara::test]
fn an_empty_pointer_asks_for_no_frame_as_it_moves() {
    assert!(
        !frame_asked_while_moving(false),
        "a pointer carrying nothing draws nothing that follows it, so moving it must leave the \
         host with nothing to paint"
    );
}

#[kithara::test]
fn a_track_the_pointer_picks_up_is_drawn_under_it() {
    let [empty, carrying, _] = carrying_pictures();

    assert!(
        carrying > empty,
        "the application says the pointer is carrying a track, so the host it is embedded in must \
         draw it: an empty pointer drew {empty} shapes, a carrying one drew {carrying}"
    );
}

#[kithara::test]
fn a_track_the_pointer_puts_down_is_taken_out_of_the_picture_again() {
    let [empty, _, put_down] = carrying_pictures();

    assert_eq!(
        put_down, empty,
        "a pointer carrying nothing must leave the picture exactly as it found it"
    );
}

#[kithara::test]
fn scene_keeps_the_public_single_redraw_signature() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(r#"Spacer(id: "scene", size: Some((w: Fill, h: Fill)))"#);
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(false), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the scene fixture must mount: {error}"));

    let scene: Result<Scene, RunError> = ui.scene();

    let _scene = scene.unwrap_or_else(|error| panic!("the compatibility scene must draw: {error}"));
}

#[kithara::test]
fn an_idle_ui_skips_its_following_frame() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(r#"Spacer(id: "idle", size: Some((w: Fill, h: Fill)))"#);
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(false), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the idle fixture must mount: {error}"));

    assert!(ui.needs_frame(), "the mounted document must paint once");
    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("the idle fixture must draw: {error}"));
    // Masonry schedules one boundary for every newly mounted tree. Completing
    // the presented frame drains it; `needs_frame` decides whether it paints.
    let _ = ui.complete_frame();
    assert!(
        !ui.needs_frame(),
        "the completed frame must leave no paint pending"
    );

    ui.frame(Duration::from_millis(16));

    assert!(
        !ui.needs_frame(),
        "an idle animation tick must not make the host rasterise and present again"
    );
}

#[kithara::test]
fn a_tick_refreshes_non_vis_reads_without_remounting() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(TickingDial { value: 0.25 }, config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the ticking knob must mount: {error}"));
    let before = geometry(
        ui.render()
            .unwrap_or_else(|error| panic!("the ticking knob must draw: {error}"))
            .scene(),
    );
    let _ = ui.complete_frame();
    assert!(!ui.needs_frame());

    ui.frame(Duration::from_millis(16));

    assert!(
        ui.needs_frame(),
        "a changed ordinary read must request paint on the tick that changed it"
    );
    let after = geometry(
        ui.render()
            .unwrap_or_else(|error| panic!("the refreshed knob must draw: {error}"))
            .scene(),
    );
    assert_ne!(
        before, after,
        "the tick changed the value but not its picture"
    );
}

#[kithara::test]
fn resize_from_one_to_two_x_keeps_layout_geometry_logical() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(r#"Spacer(id: "scaled", size: Some((w: Fill, h: Fill)))"#);
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(false), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the scale fixture must mount: {error}"));
    ui.render()
        .unwrap_or_else(|error| panic!("the 1x fixture must draw: {error}"));
    let expected = Rect {
        x: 0.0,
        y: 0.0,
        w: 240.0,
        h: 120.0,
    };
    assert_eq!(ui.rect_of("demo/scaled"), Some(expected));

    ui.resize((480, 240), 2.0);
    ui.render()
        .unwrap_or_else(|error| panic!("the 2x fixture must draw: {error}"));

    assert_eq!(
        ui.rect_of("demo/scaled"),
        Some(expected),
        "physical resize and rescale must preserve the 240x120 logical layout"
    );
}

#[kithara::test]
fn a_press_on_a_control_reaches_the_application_and_redraws_the_new_document() {
    let endpoints = Registry("fixture.lit", EndpointDesc::new(ValueKind::Bool));
    let resolver = resolver();
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut scenario = Scenario::mount(Swapper::default(), config, (240, 120), 1.0);
    assert_eq!(scenario.app().document(), "dim.klayout.ron");

    scenario.click("demo/swap");

    assert_eq!(
        scenario.published(),
        [UiEvent::Control {
            path: "demo/swap".to_owned(),
            action: ControlAction::Activate,
        }],
        "a press inside the chip must reach the application exactly once"
    );
    assert_eq!(
        scenario.app().document(),
        "lit.klayout.ron",
        "the application swapped documents, so the next frame must draw the new one"
    );
    let _scene = scenario.scene();
}

/// One control a hand can drag: how the document declares it, and the gesture
/// that must move it.
struct Draggable {
    control: &'static str,
    from: Pt,
    to: Pt,
}

/// Every control the Masonry host lets a hand drag. A control that shows a
/// value and does not answer a drag belongs here the moment it can.
const DRAGGED: [Draggable; 5] = [
    Draggable {
        control: r#"VuVertical(id: "dial", ticks: true, size: (w: Fixed(38.0), h: Fixed(120.0)), read: Model(id: "fixture.dial"))"#,
        from: Pt { x: 30.0, y: 100.0 },
        to: Pt { x: 30.0, y: 40.0 },
    },
    // The stereo meter reads across, not down: its level is the width it fills.
    Draggable {
        control: r#"VuStereo(id: "dial", size: (w: Fixed(220.0), h: Fixed(40.0)), read: Model(id: "fixture.dial"))"#,
        from: Pt { x: 20.0, y: 60.0 },
        to: Pt { x: 200.0, y: 60.0 },
    },
    Draggable {
        control: r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
        from: Pt { x: 19.0, y: 60.0 },
        to: Pt { x: 19.0, y: 40.0 },
    },
    Draggable {
        control: r#"Fader(id: "dial", size: (w: Fixed(220.0), h: Fixed(34.0)), read: Model(id: "fixture.dial"))"#,
        from: Pt { x: 20.0, y: 60.0 },
        to: Pt { x: 200.0, y: 60.0 },
    },
    Draggable {
        control: r#"Crossfader(id: "dial", ticks: true, size: (w: Fixed(220.0), h: Fixed(64.0)), read: Model(id: "fixture.dial"))"#,
        from: Pt { x: 20.0, y: 60.0 },
        to: Pt { x: 200.0, y: 60.0 },
    },
];

/// A control is dragged, not pressed. The value has to follow the pointer for
/// the whole gesture, which it can only do if the state the gesture lives in
/// survives the application acting on each step of it.
#[kithara::test]
fn dragging_a_control_moves_it_for_the_whole_gesture() {
    for control in &DRAGGED {
        let dragged = drag(control);

        assert!(
            dragged.seen.windows(2).all(|pair| pair[1] > pair[0]),
            "dragging must raise {} every step, but it read {:?}",
            control.control,
            dragged.seen
        );
    }
}

#[kithara::test]
fn dragging_a_knob_by_path_publishes_a_run_of_rising_values() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let mut scenario = Scenario::mount(
        Dial::new(false),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 120),
        1.0,
    );
    let mark = scenario.published().len();

    scenario.drag("demo/dial", Pt { x: 0.5, y: 1.0 }, Pt { x: 0.5, y: 0.0 }, 4);
    let values = scenario.published()[mark..]
        .iter()
        .filter_map(|event| match event {
            UiEvent::Control {
                action: ControlAction::SetScalar(value),
                ..
            } => Some(*value),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(values.len() > 1, "the drag must publish a run of values");
    assert!(
        values.windows(2).all(|pair| pair[1] > pair[0]),
        "dragging upward must raise every value, but it published {values:?}"
    );
}

/// A hand moving a control has to see it move. The application only reaches the
/// widget tree once the gesture ends, so a control that draws nothing but what
/// the application last said stays frozen under the pointer.
#[kithara::test]
fn a_dragged_control_redraws_on_every_step_not_only_on_release() {
    for control in &DRAGGED {
        let dragged = drag(control);

        assert!(
            dragged.drawn.windows(2).all(|pair| pair[1] != pair[0]),
            "{} drew the same frame twice while the pointer kept moving",
            control.control
        );
    }
}

/// Two controls bound to one endpoint show one value, so dragging either must
/// move both, on the step it moves — not once the hand lets go.
#[kithara::test]
fn controls_sharing_an_endpoint_move_together_during_the_gesture() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "a", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial")),
           Knob(id: "b", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(false), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the pair must mount: {error}"));
    ui.frame(Duration::from_millis(16));
    let before = geometry(
        ui.render()
            .unwrap_or_else(|error| panic!("the pair must draw: {error}"))
            .scene(),
    );

    let grip = Pt { x: 19.0, y: 60.0 };
    let moved = Pt { x: 19.0, y: 40.0 };
    ui.input(press(grip, PointerPhase::Move));
    ui.input(press(grip, PointerPhase::Down));
    ui.input(press(moved, PointerPhase::Move));
    let mid = geometry(
        ui.render()
            .unwrap_or_else(|error| panic!("the pair must draw mid-drag: {error}"))
            .scene(),
    );
    ui.input(press(moved, PointerPhase::Up));
    let after = geometry(
        ui.render()
            .unwrap_or_else(|error| panic!("the pair must draw after: {error}"))
            .scene(),
    );

    assert_ne!(
        before, mid,
        "the drag must show while the hand is still down"
    );
    assert_eq!(
        mid, after,
        "the second knob only caught up once the hand let go: the frame changed \
         after the gesture ended rather than during it"
    );
}

/// A double click resets a knob. The two presses are one gesture, so whatever
/// the first one does must not throw away the state the second one is measured
/// against.
#[kithara::test]
fn a_double_click_resets_the_knob_it_lands_on() {
    let endpoints = Registry("fixture.dial", EndpointDesc::new(ValueKind::Scalar));
    let resolver = one_control(
        r#"Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)), read: Model(id: "fixture.dial"))"#,
    );
    let config = Config::builder()
        .endpoints(&endpoints)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Dial::new(false), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the dial must mount: {error}"));
    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("the dial must draw: {error}"));

    // Drag it away from its reset point first, so the reset is visible.
    let grip = Pt { x: 19.0, y: 60.0 };
    ui.input(press(grip, PointerPhase::Move));
    ui.input(press(grip, PointerPhase::Down));
    ui.input(press(Pt { x: 19.0, y: 40.0 }, PointerPhase::Move));
    ui.input(press(Pt { x: 19.0, y: 40.0 }, PointerPhase::Up));
    let moved = ui.app().value;
    assert_ne!(
        moved, 0.5,
        "the drag must move the knob off its reset point"
    );

    ui.input(press(grip, PointerPhase::Down));
    ui.input(press(grip, PointerPhase::Up));
    ui.input(Input::Pointer(PointerInput::new(
        MOUSE,
        None,
        PointerPhase::Down,
        Some(grip),
        2,
    )));

    assert_eq!(
        ui.app().value,
        0.5,
        "a double click must put the knob back, but it read {moved} then {}",
        ui.app().value
    );
}

/// Isolates the question the failing test above cannot answer on its own: does
/// the press reach the leaf at all, or is it the app layer that loses it?
#[kithara::test]
fn the_masonry_root_under_the_app_layer_publishes_the_same_press() {
    use kithara_platform::sync::Arc;
    use masonry::{
        app::{RenderRootOptions, WindowSizePolicy},
        core::PointerEvent,
        dpi::{PhysicalPosition, PhysicalSize},
        theme::default_property_set,
        ui_events::pointer::{
            PointerButton, PointerButtonEvent, PointerButtons, PointerId, PointerInfo,
            PointerState, PointerType,
        },
    };

    use crate::{
        compile::compile,
        render::{document, masonry::MasonryHost},
        source::UiConfig,
    };

    let endpoints = Registry("fixture.lit", EndpointDesc::new(ValueKind::Bool));
    let resolver = resolver();
    let reads = Swapper::default();
    let ui = compile(
        "dim.klayout.ron",
        &resolver,
        &endpoints,
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
    )
    .unwrap_or_else(|error| panic!("fixture must compile: {error}"));
    let ctx = Ctx::new(&ui, &reads, builtin::skin_doc(), Clock::default());
    let host = MasonryHost::new(ctx, skin());
    let node = document::render(&ui.root, ctx, host);
    let mut root = crate::render::masonry::MasonryRoot::new(
        node,
        RenderRootOptions {
            default_properties: Arc::new(default_property_set()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size: PhysicalSize::new(240, 120),
            scale_factor: 1.0,
            test_font: None,
        },
    )
    .unwrap_or_else(|error| panic!("fixture must mount: {error}"));
    root.redraw()
        .unwrap_or_else(|error| panic!("fixture must draw: {error}"));

    let mut buttons = PointerButtons::new();
    buttons.insert(PointerButton::Primary);
    root.handle_pointer_event(PointerEvent::Down(PointerButtonEvent {
        button: Some(PointerButton::Primary),
        pointer: PointerInfo {
            pointer_id: Some(PointerId::PRIMARY),
            persistent_device_id: None,
            pointer_type: PointerType::Mouse,
        },
        state: PointerState {
            position: PhysicalPosition::new(60.0, 60.0),
            buttons,
            count: 1,
            scale_factor: 1.0,
            ..PointerState::default()
        },
    }))
    .unwrap_or_else(|error| panic!("press must stay typed: {error}"));

    assert_eq!(
        root.take_actions(),
        vec![UiEvent::Control {
            path: "demo/swap".to_owned(),
            action: ControlAction::Activate,
        }],
        "the masonry root must publish the press"
    );
}

/// The retained host owns its frame count. Driving it is what makes a frame
/// reproducible, so the count has to be the host's and not a wall clock's.
#[kithara::test]
fn each_frame_advances_the_host_clock_by_one() {
    let (registry, resolver) = tree_fixture();
    let config = Config::builder()
        .endpoints(&registry)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Typed::default(), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the fixture must mount: {error}"));
    assert_eq!(ui.clock().frame, 0);
    ui.frame(Duration::from_millis(16));
    assert_eq!(ui.clock().frame, 1);
    ui.frame(Duration::from_millis(16));
    assert_eq!(ui.clock().frame, 2);
}

/// Elapsed time is the sum of the steps it was driven with, so a caller that
/// hands the same steps twice gets the same reading twice.
#[kithara::test]
fn the_host_clock_accumulates_the_steps_it_was_driven_with() {
    let (registry, resolver) = tree_fixture();
    let config = Config::builder()
        .endpoints(&registry)
        .resolver(&resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Typed::default(), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the fixture must mount: {error}"));
    for _ in 0..4 {
        ui.frame(Duration::from_millis(25));
    }
    assert_eq!(ui.clock().elapsed, Duration::from_millis(100));
}

/// A registry that answers for whatever tab a page row binds to, so a fixture
/// can name its rows without listing them twice.
struct Tabs(EndpointDesc);

impl Default for Tabs {
    fn default() -> Self {
        Self(EndpointDesc::new(ValueKind::Bool))
    }
}

impl EndpointRegistry for Tabs {
    fn endpoint(&self, category: EndpointCategory, _id: &EndpointId) -> Option<&EndpointDesc> {
        (category == EndpointCategory::Model).then_some(&self.0)
    }
}

/// An application showing a page list, recording which page was asked for.
#[derive(Default)]
struct Pages {
    opened: Vec<String>,
}

impl Reads for Pages {
    fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
        Some(ReadValue::Bool(false))
    }
}

impl App for Pages {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "pages.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { path, action } = event
            && action == ControlAction::Activate
        {
            self.opened.push(path);
        }
    }
}

/// A window holding more page rows than it can show at once, each row a module
/// of its own.
///
/// The module is named `gallery-nav` because that name is on the list of
/// modules whose contents an engine drives from above, and each row is an
/// `Include` so that the rows the engine drives stand in nodes of their own -
/// which is what a window moves when it scrolls.
fn page_list(rows: usize) -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "pages.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "pages",
            root: Module(instance: "pages", source: "pages.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    let items = (0..rows)
        .map(|row| {
            format!(
                r#"Include(id: "row{row}", source: "row.kmodule.ron",
                    with: {{ "label": "ROW {row}", "tab": "page.row{row}" }}),"#
            )
        })
        .collect::<String>();
    resolver.insert(
        "pages.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "gallery-nav", chrome: Plain,
                root: Scroll(id: "list", size: (w: Fill, h: Fill),
                    child: Column(children: [{items}])))"#
        ),
    );
    resolver.insert(
        "row.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "gallery-nav-row",
            parameters: ["label", "tab"],
            root: NavItem(id: "item", label: "$label", icon: "Disc", read: Model(id: "$tab")))"#,
    );
    resolver
}

/// A row a window had to scroll to show answers the press it is standing
/// under, not the press aimed at where it used to stand.
#[kithara::test]
fn a_row_a_window_scrolled_into_view_answers_its_own_press() {
    let rows = 20;
    let last = format!("pages/row{}/item", rows - 1);
    let endpoints = Tabs::default();
    let resolver = page_list(rows);
    let mut scenario = Scenario::mount(
        Pages::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 400),
        1.0,
    );
    let standing = scenario.rect_of(&last);
    assert!(
        standing.is_none_or(|rect| rect.y >= 400.0),
        "the fixture must hold more rows than the window shows"
    );
    for _ in 0..8 {
        scenario.wheel("pages/row9/item", -1.0);
        scenario.scene();
    }
    scenario.click(&last);
    assert_eq!(scenario.app().opened, vec![last]);
}

/// An application showing one reading, which either keeps moving - the way a
/// meter fed from somewhere else does - or settles and stays put.
struct Reading {
    moves: bool,
    shown: String,
}

impl Reading {
    fn new(moves: bool) -> Self {
        Self {
            moves,
            shown: String::from("0"),
        }
    }
}

impl Reads for Reading {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.reading").then_some(ReadValue::Text(&self.shown))
    }
}

impl App for Reading {
    fn skin(&self) -> &Skin {
        skin()
    }

    fn document(&self) -> &str {
        "one.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn tick(&mut self) {
        if !self.moves {
            return;
        }
        let next = self.shown.parse::<u32>().unwrap_or_default() + 1;
        self.shown = next.to_string();
    }

    fn update(&mut self, _event: UiEvent) {}
}

/// The parts a document holding one reading is mounted from.
fn reading_fixture() -> (Registry, MemResolver) {
    (
        Registry("fixture.reading", EndpointDesc::new(ValueKind::Text)),
        one_control(
            r#"Readout(id: "reading", label: Some("READING"), read: Model(id: "fixture.reading"))"#,
        ),
    )
}

/// Mounts that document, drawn and settled, so a test acts on a window that
/// has come to rest.
fn reading_ui<'a>(
    moves: bool,
    endpoints: &'a Registry,
    resolver: &'a MemResolver,
) -> Ui<'a, Reading> {
    let config = Config::builder()
        .endpoints(endpoints)
        .resolver(resolver)
        .text(builtin::text_doc())
        .build();
    let mut ui = Ui::new(Reading::new(moves), config, (240, 120), 1.0)
        .unwrap_or_else(|error| panic!("the reading must mount: {error}"));
    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("the reading must draw its first frame: {error}"));
    let _ = ui.complete_frame();
    ui
}

/// A document showing values the application keeps changing draws itself
/// again on its own, instead of only when something unrelated wakes it.
#[kithara::test]
fn a_frame_that_moved_a_value_asks_for_the_frame_after_it() {
    let (endpoints, resolver) = reading_fixture();
    let mut ui = reading_ui(true, &endpoints, &resolver);

    ui.frame(Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("the moved reading must draw: {error}"));

    assert!(ui.complete_frame());
}

/// A document that has come to rest lets the window sleep, so a page showing
/// nothing new costs nothing.
#[kithara::test]
fn a_frame_that_moved_nothing_asks_for_no_frame_after_it() {
    let (endpoints, resolver) = reading_fixture();
    let mut ui = reading_ui(false, &endpoints, &resolver);

    ui.frame(Duration::from_millis(16));

    assert!(!ui.complete_frame());
}
