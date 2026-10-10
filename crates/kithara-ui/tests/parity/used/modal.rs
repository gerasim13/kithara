//! What the two hosts owe each other about a modal: the scrim it lays over the
//! whole window, the surface it centres on it, and the input it keeps.
//!
//! The page under the modal holds a pressable and a knob, so a press the scrim
//! failed to take would show up as a write the page published.

use iced::{Background, Color, Rectangle, Vector, advanced::renderer::Quad};
use kithara_test_utils::kithara;
use kithara_ui::{
    app::{App, Config, Ui},
    backends::paint_color,
    builtin,
    compile::{CompiledUi, compile},
    draw::{Pt, Rect, Rgba},
    ids::EndpointId,
    interact::{Input, InputMethod, Key, MOUSE, Modifiers, PointerInput, PointerPhase, Scroll},
    registry::{EndpointCategory, EndpointDesc, EndpointRegistry, ValueKind},
    render::{ReadValue, Reads, Scope, Skin, UiEvent, WindowCommand, WindowEdge, WriteValue},
    skin::ColorRole,
    source::{MemResolver, UiConfig},
    view,
};

use super::press::trigger;
use crate::immediate::Immediate;

/// The window both hosts open the page in.
const WINDOW: (u32, u32) = (480, 320);

/// The page: a knob and a search field in a strip along the top and a
/// pressable filling the rest. `{modal}` stands first, so a modal that took
/// room would push them all down.
const PAGE: &str = r#"Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
    {modal}
    Row(size: (w: Fill, h: Fixed(60.0)), gap: 0.0, pad: 0.0, children: [
        Knob(id: "dial", size: (w: Fixed(38.0), h: Fixed(49.0)),
            read: Model(id: "fixture.dial"), write: Parameter(id: "fixture.dial")),
        Search(id: "query", size: (w: Fixed(160.0), h: Fixed(26.0)),
            read: Model(id: "fixture.query"), write: Command(id: "fixture.query")),
    ]),
    Pressable(id: "page", press: Command(id: "fixture.page"),
        child: Spacer(id: "page-face", size: Some((w: Fill, h: Fill)))),
])"#;

/// A page whose layout mounts one module, `demo`, with `root` as its root.
fn page(module_id: &str, root: &str, resize_edges: bool) -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "page.klayout.ron",
        &format!(
            r#"(schema: "kithara.layout", version: 1, id: "page", resize_edges: {resize_edges},
                root: Module(instance: "demo", source: "page.kmodule.ron", size: (w: Fill, h: Fill)))"#
        ),
    );
    resolver.insert(
        "page.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "{module_id}", chrome: Plain,
                root: {root})"#
        ),
    );
    resolver
}

/// A column holding `children` above the page pressable.
fn over_page(children: &str) -> String {
    format!(
        r#"Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            {children}
            Pressable(id: "page", press: Command(id: "fixture.page"),
                child: Spacer(id: "page-face", size: Some((w: Fill, h: Fill)))),
        ])"#
    )
}

/// A modal over the page, its content a quiet strip above a pressable row.
fn modal(width: f32, height: f32) -> String {
    format!(
        r#"Modal(id: "settings", open: Model(id: "fixture.open"),
            close: Command(id: "fixture.close"),
            content: Column(id: "surface", size: (w: Fixed({width:.1}), h: Fixed({height:.1})),
                gap: 0.0, pad: 0.0, children: [
                    Spacer(id: "inside", size: Some((w: Fill, h: Fixed(40.0)))),
                    Pressable(id: "pick", press: Command(id: "fixture.pick"),
                        child: Spacer(id: "pick-face", size: Some((w: Fill, h: Fixed(20.0))))),
                ])),"#
    )
}

/// A modal whose content is a header that shuts it above a list taller
/// than the box it scrolls in.
const LISTING: &str = r#"Modal(id: "settings", open: Model(id: "fixture.open"),
    close: Command(id: "fixture.close"),
    content: Column(id: "surface", size: (w: Fixed(200.0), h: Fixed(100.0)),
        gap: 0.0, pad: 0.0, children: [
            Pressable(id: "header", press: Command(id: "fixture.shut"),
                child: Spacer(id: "header-face", size: Some((w: Fill, h: Fixed(20.0))))),
            Scroll(id: "list", size: (w: Fill, h: Fixed(80.0)),
                child: Column(gap: 0.0, pad: 0.0, children: [
                    Pressable(id: "row0", press: Command(id: "fixture.row0"),
                        child: Spacer(id: "row0-face", size: Some((w: Fill, h: Fixed(40.0))))),
                    Pressable(id: "row1", press: Command(id: "fixture.row1"),
                        child: Spacer(id: "row1-face", size: Some((w: Fill, h: Fixed(40.0))))),
                    Pressable(id: "row2", press: Command(id: "fixture.row2"),
                        child: Spacer(id: "row2-face", size: Some((w: Fill, h: Fixed(40.0))))),
                    Pressable(id: "row3", press: Command(id: "fixture.row3"),
                        child: Spacer(id: "row3-face", size: Some((w: Fill, h: Fixed(40.0))))),
                    Pressable(id: "row4", press: Command(id: "fixture.row4"),
                        child: Spacer(id: "row4-face", size: Some((w: Fill, h: Fixed(40.0))))),
                ])),
        ])),"#;

/// The listing modal with a button between the header and the list, the kind
/// of control an engine drives in a hosted module.
fn hosted_listing() -> String {
    LISTING
        .replace("h: Fixed(100.0)", "h: Fixed(126.0)")
        .replace(
            "            Scroll(id: \"list\"",
            "            Button(id: \"save\", label: \"SAVE\", write: Command(id: \"fixture.save\"),\n                size: (w: Fill, h: Fixed(26.0))),\n            Scroll(id: \"list\"",
        )
}

/// What the page holds: no modal, a modal whose content asks for a size, or
/// the listing modal, the last in a module whose input an engine owns.
#[derive(Clone, Copy, Debug)]
enum Holds {
    Nothing,
    Modal(f32, f32),
    Listing,
    HostedListing,
}

impl Holds {
    const SMALL: Self = Self::Modal(100.0, 60.0);
    const OVERSIZE: Self = Self::Modal(600.0, 400.0);

    fn documents(self) -> MemResolver {
        let modal = match self {
            Self::Nothing => String::new(),
            Self::Modal(width, height) => modal(width, height),
            Self::Listing => LISTING.to_owned(),
            Self::HostedListing => hosted_listing(),
        };
        let module = match self {
            Self::HostedListing => "app-bar",
            Self::Nothing | Self::Modal(..) | Self::Listing => "page",
        };
        page(module, &PAGE.replace("{modal}", &modal), false)
    }
}

/// The geometry a 100x60 content column takes in the window: one frame pixel
/// around it, the whole surface centred.
mod small {
    use super::Rect;

    pub(super) const SURFACE: Rect = Rect {
        x: 189.0,
        y: 129.0,
        w: 102.0,
        h: 62.0,
    };
    pub(super) const CONTENT: Rect = Rect {
        x: 190.0,
        y: 130.0,
        w: 100.0,
        h: 60.0,
    };
}

/// The geometry content larger than the window shrinks to: the window less
/// the reach of the shadow on every side, so the whole shadow stays in it.
mod oversize {
    use super::Rect;

    pub(super) const SURFACE: Rect = Rect {
        x: 80.0,
        y: 104.0,
        w: 320.0,
        h: 112.0,
    };
    pub(super) const CONTENT: Rect = Rect {
        x: 81.0,
        y: 105.0,
        w: 318.0,
        h: 110.0,
    };
}

/// The modal's look in the dark skin, as the settings window of the design
/// canon draws it.
mod look {
    pub(super) const SCRIM_ALPHA: f32 = 0.72;
    pub(super) const SHADOW_ALPHA: f32 = 0.55;
    pub(super) const SHADOW_BLUR: f32 = 80.0;
    pub(super) const SHADOW_OFFSET_Y: f32 = 24.0;
    pub(super) const TICK_SIZE: f32 = 10.0;
    pub(super) const TICK_WIDTH: f32 = 2.0;
    pub(super) const BORDER: f32 = 1.0;
}

fn role(role: ColorRole) -> Rgba {
    builtin::skin().palette[role]
}

fn faded(role_: ColorRole, alpha: f32) -> Rgba {
    Rgba {
        a: alpha,
        ..role(role_)
    }
}

/// The application: it answers the modal's flag and keeps every event the
/// document publishes, so what a gesture reached is what it published.
#[derive(Default)]
struct Page {
    open: bool,
    /// Whether the close the modal writes, or its header's press, shuts it.
    shuts: bool,
    published: Vec<UiEvent>,
    query: String,
}

impl Page {
    fn open() -> Self {
        Self::with(true)
    }

    fn with(open: bool) -> Self {
        Self {
            open,
            ..Self::default()
        }
    }
}

impl Reads for Page {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        match Scope::split(endpoint).0 {
            "fixture.open" => Some(ReadValue::Bool(self.open)),
            "fixture.dial" => Some(ReadValue::Scalar(0.5)),
            "fixture.query" => Some(ReadValue::Text(&self.query)),
            _ => None,
        }
    }
}

impl App for Page {
    fn document(&self) -> &str {
        "page.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn skin(&self) -> &Skin {
        builtin::skin()
    }

    /// The first query typed opens the modal, so the field it was typed into
    /// still holds the keyboard when the modal stands.
    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Write {
            key,
            value: WriteValue::Text(query),
        } = &event
            && key == "fixture.query"
        {
            self.query.clone_from(query);
            self.open = true;
        }
        if self.shuts && [trigger("fixture.close"), trigger("fixture.shut")].contains(&event) {
            self.open = false;
        }
        self.published.push(event);
    }
}

struct Endpoints {
    flag: EndpointDesc,
    scalar: EndpointDesc,
    text: EndpointDesc,
    trigger: EndpointDesc,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            flag: EndpointDesc::new(ValueKind::Bool),
            scalar: EndpointDesc::new(ValueKind::Scalar),
            text: EndpointDesc::new(ValueKind::Text),
            trigger: EndpointDesc::new(ValueKind::Trigger),
        }
    }
}

impl EndpointRegistry for Endpoints {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        match (category, id.0.as_str()) {
            (EndpointCategory::Model, "fixture.open") => Some(&self.flag),
            (EndpointCategory::Model, "fixture.dial")
            | (EndpointCategory::Parameter, "fixture.dial") => Some(&self.scalar),
            (EndpointCategory::Model | EndpointCategory::Command, "fixture.query") => {
                Some(&self.text)
            }
            (
                EndpointCategory::Command,
                "fixture.close" | "fixture.page" | "fixture.pick" | "fixture.shut" | "fixture.save"
                | "fixture.burger" | "fixture.row0" | "fixture.row1" | "fixture.row2"
                | "fixture.row3" | "fixture.row4",
            ) => Some(&self.trigger),
            _ => None,
        }
    }
}

fn query(text: &str) -> UiEvent {
    UiEvent::Write {
        key: "fixture.query".to_owned(),
        value: WriteValue::Text(text.to_owned()),
    }
}

fn wrote(events: &[UiEvent], to: &str) -> bool {
    events
        .iter()
        .any(|event| matches!(event, UiEvent::Write { key, .. } if key == to))
}

fn centre(rect: Rect) -> Pt {
    Pt {
        x: rect.x + rect.w / 2.0,
        y: rect.y + rect.h / 2.0,
    }
}

/// One step of a gesture, played the same way on both hosts.
#[derive(Clone, Copy)]
enum Step {
    Click(Pt),
    /// Presses at the first point, travels to the second and lets go there.
    Drag(Pt, Pt),
    Escape,
    /// Types one character, the key that types it named for the immediate host.
    Type(&'static str, iced::keyboard::key::Code),
    /// An input method commits text, as a paste reaches a field.
    Commit(&'static str),
    /// Notches of the wheel over a point, the pointer arriving there first.
    Wheel(Pt, f32),
    /// Shift comes to be held.
    Shift,
}

/// Mounts the page on the retained host and hands it to the check.
fn with_retained<R>(
    resolver: &MemResolver,
    app: Page,
    check: impl FnOnce(&mut Ui<'_, Page>) -> R,
) -> R {
    let endpoints = Endpoints::default();
    let mut ui = Ui::new(
        app,
        Config::builder()
            .endpoints(&endpoints)
            .resolver(resolver)
            .text(builtin::text_doc())
            .build(),
        WINDOW,
        1.0,
    )
    .unwrap_or_else(|error| panic!("the page must mount on the retained host: {error}"));
    check(&mut ui)
}

fn laid(ui: &Ui<'_, Page>, path: &str) -> Rect {
    ui.rect_of(path)
        .unwrap_or_else(|| panic!("{path} must be laid out"))
}

fn pointer(ui: &mut Ui<'_, Page>, phase: PointerPhase, at: Pt) {
    ui.input(Input::Pointer(PointerInput::new(
        MOUSE,
        None,
        phase,
        Some(at),
        1,
    )));
}

fn play_retained(ui: &mut Ui<'_, Page>, steps: &[Step]) {
    for step in steps {
        match *step {
            Step::Click(at) => {
                for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
                    pointer(ui, phase, at);
                }
            }
            Step::Drag(from, to) => {
                pointer(ui, PointerPhase::Move, from);
                pointer(ui, PointerPhase::Down, from);
                pointer(ui, PointerPhase::Move, to);
                pointer(ui, PointerPhase::Up, to);
            }
            Step::Escape => {
                ui.input(Input::KeyPressed {
                    key: Key::Escape,
                    modifiers: Modifiers::default(),
                    text: None,
                });
                ui.input(Input::KeyReleased {
                    key: Key::Escape,
                    modifiers: Modifiers::default(),
                });
            }
            Step::Type(text, _) => {
                ui.input(Input::KeyPressed {
                    key: Key::character(text, None),
                    modifiers: Modifiers::default(),
                    text: Some(text),
                });
                ui.input(Input::KeyReleased {
                    key: Key::character(text, None),
                    modifiers: Modifiers::default(),
                });
            }
            Step::Commit(text) => {
                ui.input(Input::InputMethod(InputMethod::Commit(text)));
            }
            Step::Wheel(at, notches) => {
                pointer(ui, PointerPhase::Move, at);
                ui.input(Input::Wheel(Scroll::Lines { x: 0.0, y: notches }));
            }
            Step::Shift => {
                ui.input(Input::ModifiersChanged(Modifiers::new(
                    false, false, false, true,
                )));
            }
        }
    }
}

fn compiled(resolver: &MemResolver) -> CompiledUi {
    compile(
        "page.klayout.ron",
        resolver,
        &Endpoints::default(),
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
        &view::EMPTY,
    )
    .unwrap_or_else(|error| panic!("the page must compile: {error}"))
}

fn play_immediate(resolver: &MemResolver, app: Page, steps: &[Step]) -> Vec<UiEvent> {
    let ui = compiled(resolver);
    let mut host = Immediate::mount(app, &ui, builtin::skin(), WINDOW);
    for step in steps {
        match *step {
            Step::Click(at) => {
                host.click_at(at);
            }
            Step::Drag(from, to) => {
                host.press_at(from);
                host.hover_at(to);
                host.release_at(to);
            }
            Step::Escape => {
                host.key_at(
                    Pt { x: 1.0, y: 1.0 },
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    iced::keyboard::key::Code::Escape,
                );
            }
            Step::Type(text, code) => {
                host.key_at(
                    Pt { x: 1.0, y: 1.0 },
                    iced::keyboard::Key::Character(text.into()),
                    code,
                );
            }
            Step::Commit(text) => {
                host.commit_at(Pt { x: 1.0, y: 1.0 }, text);
            }
            Step::Wheel(at, notches) => {
                host.wheel_at(at, notches);
            }
            Step::Shift => {
                host.modifiers_at(Pt { x: 1.0, y: 1.0 }, iced::keyboard::Modifiers::SHIFT);
            }
        }
    }
    host.app().published.clone()
}

/// What each host published for the gesture, retained first.
fn both(resolver: &MemResolver, app: impl Fn() -> Page, steps: &[Step]) -> [Vec<UiEvent>; 2] {
    let retained = with_retained(resolver, app(), |ui| {
        play_retained(ui, steps);
        ui.app().published.clone()
    });
    [retained, play_immediate(resolver, app(), steps)]
}

/// Both hosts published `expected` for the gesture.
fn assert_both(
    resolver: &MemResolver,
    open: bool,
    steps: &[Step],
    expected: &[UiEvent],
    what: &str,
) {
    let [retained, immediate] = both(resolver, || Page::with(open), steps);
    assert_eq!(retained, expected, "the retained host: {what}");
    assert_eq!(immediate, expected, "the immediate host: {what}");
}

/// Where the page's controls stand on the retained host, with nothing over
/// them.
fn page_points() -> (Pt, Pt) {
    with_retained(&Holds::Nothing.documents(), Page::default(), |ui| {
        let face = laid(ui, "demo/page-face");
        let face = Pt {
            x: face.x + face.w - 20.0,
            y: face.y + face.h - 20.0,
        };
        (face, centre(laid(ui, "demo/dial")))
    })
}

/// The quads the immediate host draws for the page.
fn immediate_quads(holds: Holds, open: bool) -> Vec<(Rectangle, Quad, Background)> {
    let ui = compiled(&holds.documents());
    let mut host = Immediate::mount(Page::with(open), &ui, builtin::skin(), WINDOW);
    host.quads()
}

fn filled(quads: &[(Rectangle, Quad, Background)], bounds: Rect, color: Rgba) -> bool {
    quads.iter().any(|(_, quad, background)| {
        Rect::from(quad.bounds) == bounds && *background == Background::Color(color.into())
    })
}

/// The four bars of the two corner ticks a surface carries, top-left and
/// bottom-right, each lying over the frame.
fn ticks(surface: Rect) -> [Rect; 4] {
    let (size, width) = (look::TICK_SIZE, look::TICK_WIDTH);
    let right = surface.x + surface.w;
    let bottom = surface.y + surface.h;
    [
        Rect {
            x: surface.x,
            y: surface.y,
            w: size,
            h: width,
        },
        Rect {
            x: surface.x,
            y: surface.y,
            w: width,
            h: size,
        },
        Rect {
            x: right - size,
            y: bottom - width,
            w: size,
            h: width,
        },
        Rect {
            x: right - width,
            y: bottom - size,
            w: width,
            h: size,
        },
    ]
}

/// Where the shadow of a surface inks: offset down and spread by its blur.
fn shadow_ink(surface: Rect) -> Rect {
    Rect {
        x: surface.x - look::SHADOW_BLUR,
        y: surface.y + look::SHADOW_OFFSET_Y - look::SHADOW_BLUR,
        w: surface.w + look::SHADOW_BLUR * 2.0,
        h: surface.h + look::SHADOW_BLUR * 2.0,
    }
}

fn covers(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

/// The surface quad the immediate host drew, carrying frame and shadow.
fn surface_quad(quads: &[(Rectangle, Quad, Background)], surface: Rect) -> (Rect, Quad) {
    quads
        .iter()
        .find(|(_, quad, background)| {
            Rect::from(quad.bounds) == surface
                && *background == Background::Color(role(ColorRole::BgPanel).into())
        })
        .map(|(layer, quad, _)| (Rect::from(*layer), *quad))
        .unwrap_or_else(|| panic!("no surface quad at {surface:?} in {quads:#?}"))
}

fn assert_immediate_draws(holds: Holds, surface: Rect) {
    let quads = immediate_quads(holds, true);
    let window = Rect {
        x: 0.0,
        y: 0.0,
        w: 480.0,
        h: 320.0,
    };

    assert!(
        filled(&quads, window, faded(ColorRole::BgDeep, look::SCRIM_ALPHA)),
        "the scrim must cover the whole window: {quads:#?}"
    );
    let (layer, quad) = surface_quad(&quads, surface);
    assert_eq!(quad.border.width, look::BORDER);
    assert_eq!(quad.border.color, Color::from(role(ColorRole::Line)));
    assert_eq!(
        quad.shadow.color,
        Color::from(faded(ColorRole::Shadow, look::SHADOW_ALPHA))
    );
    assert_eq!(quad.shadow.offset, Vector::new(0.0, look::SHADOW_OFFSET_Y));
    assert_eq!(quad.shadow.blur_radius, look::SHADOW_BLUR);
    let ink = shadow_ink(surface);
    assert!(
        covers(layer, ink),
        "the shadow inks {ink:?}, outside the layer {layer:?} it was drawn in"
    );
    assert!(
        covers(window, ink),
        "the shadow inks {ink:?}, outside the window"
    );
    for tick in ticks(surface) {
        assert!(
            filled(&quads, tick, role(ColorRole::Accent)),
            "no corner tick at {tick:?}"
        );
    }
}

fn packed(color: Rgba) -> u32 {
    paint_color(color).premultiply().to_rgba8().to_u32()
}

/// The colour words of the retained host's picture.
fn drawn(ui: &mut Ui<'_, Page>) -> Vec<u32> {
    ui.scene()
        .unwrap_or_else(|error| panic!("the retained host must draw: {error}"))
        .encoding()
        .draw_data
        .clone()
}

/// How many times the retained host's picture names one colour.
fn painted(ui: &mut Ui<'_, Page>, color: Rgba) -> usize {
    let word = packed(color);
    drawn(ui).iter().filter(|drawn| **drawn == word).count()
}

fn assert_retained_draws(holds: Holds, content: Rect) {
    let colors = [
        ("scrim", faded(ColorRole::BgDeep, look::SCRIM_ALPHA)),
        ("background", role(ColorRole::BgPanel)),
        ("frame", role(ColorRole::Line)),
        ("ticks", role(ColorRole::Accent)),
        ("shadow", faded(ColorRole::Shadow, look::SHADOW_ALPHA)),
    ];
    let resolver = holds.documents();
    let hidden: Vec<usize> = with_retained(&resolver, Page::default(), |ui| {
        colors
            .iter()
            .map(|(_, color)| painted(ui, *color))
            .collect()
    });
    with_retained(&resolver, Page::open(), |ui| {
        assert_eq!(
            ui.rect_of("demo/inside"),
            Some(Rect { h: 40.0, ..content }),
            "the content's first row must stand at the top of the content, centred inside its \
             frame"
        );
        for ((name, color), before) in colors.iter().zip(&hidden) {
            assert!(
                painted(ui, *color) > *before,
                "the shown modal must paint its {name}"
            );
        }
    });
}

/// A shown modal centres its surface on the window and draws the scrim, the
/// frame, the corner ticks and the shadow, at the same geometry on both hosts.
#[kithara::test]
fn a_shown_modal_draws_its_scrim_surface_ticks_and_shadow_on_the_immediate_host() {
    assert_immediate_draws(Holds::SMALL, small::SURFACE);
}

#[kithara::test]
fn a_shown_modal_draws_its_scrim_surface_ticks_and_shadow_on_the_retained_host() {
    assert_retained_draws(Holds::SMALL, small::CONTENT);
}

/// Content larger than the window shrinks to the window less the shadow's
/// reach, so the surface stays centred and the shadow is drawn whole.
#[kithara::test]
fn oversize_content_shrinks_to_the_window_with_its_shadow_whole() {
    assert_immediate_draws(Holds::OVERSIZE, oversize::SURFACE);
    assert_retained_draws(Holds::OVERSIZE, oversize::CONTENT);
}

/// A gesture on the scrim reaches nothing under it and leaves the modal open,
/// in a plain module and in one whose input an engine owns, though with the
/// modal shut each
/// reaches the control it lands on.
#[kithara::test]
fn a_gesture_on_the_scrim_reaches_nothing_under_it() {
    let (face, dial) = page_points();
    let up = Pt {
        x: dial.x,
        y: dial.y - 20.0,
    };
    let gestures: [(&str, Step, &str); 3] = [
        ("press", Step::Click(face), "fixture.page"),
        ("drag", Step::Drag(dial, up), "fixture.dial"),
        ("wheel", Step::Wheel(dial, -2.0), "fixture.dial"),
    ];
    for holds in [Holds::SMALL, Holds::HostedListing] {
        let resolver = holds.documents();
        for (name, step, under) in &gestures {
            let [retained, immediate] = both(&resolver, Page::default, &[*step]);
            assert!(
                wrote(&retained, under) && wrote(&immediate, under),
                "with the modal shut the {name} in {holds:?} reaches {under}: {retained:?} \
                 {immediate:?}"
            );
            assert_both(
                &resolver,
                true,
                &[*step],
                &[],
                &format!("{name} in {holds:?}"),
            );
        }
    }
}

/// Escape writes the close binding wherever the pointer rests.
#[kithara::test]
fn escape_closes_the_modal() {
    assert_both(
        &Holds::SMALL.documents(),
        true,
        &[Step::Escape],
        &[trigger("fixture.close")],
        "escape",
    );
}

/// A press inside the content belongs to the content: on a quiet part of it
/// nothing is written, and on its pressable row that row's binding is.
#[kithara::test]
fn a_press_inside_the_modal_reaches_its_content_and_never_closes_it() {
    let resolver = Holds::SMALL.documents();
    let (inside, pick) = with_retained(&resolver, Page::open(), |ui| {
        (
            centre(laid(ui, "demo/inside")),
            centre(laid(ui, "demo/pick-face")),
        )
    });

    assert_both(
        &resolver,
        true,
        &[Step::Click(inside), Step::Click(pick)],
        &[trigger("fixture.pick")],
        "press inside",
    );
}

/// The header's press publishes the header's own write and not the close, a
/// button an engine drives publishes its own, and the wheel over the list
/// scrolls it: the row under the list's top edge after the notches is a later
/// row than the one standing there before. A plain and a hosted module alike.
#[kithara::test]
fn presses_and_the_wheel_inside_the_modal_reach_its_content() {
    for (holds, button) in [(Holds::Listing, false), (Holds::HostedListing, true)] {
        let resolver = holds.documents();
        let (header, top, save) = with_retained(&resolver, Page::open(), |ui| {
            let first = laid(ui, "demo/row0-face");
            let top = Pt {
                x: first.x + first.w / 2.0,
                y: first.y + 10.0,
            };
            let save = button.then(|| centre(laid(ui, "demo/save")));
            (centre(laid(ui, "demo/header-face")), top, save)
        });

        let what = |name: &str| format!("{name} in {holds:?}");
        assert_both(
            &resolver,
            true,
            &[Step::Click(header)],
            &[trigger("fixture.shut")],
            &what("header"),
        );
        if let Some(save) = save {
            assert_both(
                &resolver,
                true,
                &[Step::Click(save)],
                &[trigger("fixture.save")],
                &what("button"),
            );
        }
        assert_both(
            &resolver,
            true,
            &[Step::Click(top)],
            &[trigger("fixture.row0")],
            &what("unscrolled list"),
        );

        let steps = [Step::Wheel(top, -2.0), Step::Click(top)];
        let [retained, immediate] = both(&resolver, Page::open, &steps);
        for (host, events) in [("retained", &retained), ("immediate", &immediate)] {
            assert!(
                matches!(events.as_slice(), [UiEvent::Write { key, .. }]
                    if key.starts_with("fixture.row") && key != "fixture.row0"),
                "the {host} list in {holds:?} must scroll under the wheel: {events:?}"
            );
        }
        assert_eq!(
            retained, immediate,
            "both hosts scroll the list alike in {holds:?}"
        );
    }
}

/// A modal its flag holds shut draws nothing, takes no room and no press: the
/// page is the page it would be with no modal in it.
#[kithara::test]
fn a_hidden_modal_leaves_the_page_as_if_it_were_not_there() {
    let (face, _) = page_points();
    let resolver = Holds::SMALL.documents();
    let seen = |ui: &mut Ui<'_, Page>| (ui.rect_of("demo/dial"), drawn(ui));
    let bare = with_retained(&Holds::Nothing.documents(), Page::default(), seen);
    let shut = with_retained(&resolver, Page::default(), seen);
    assert_eq!(shut.0, bare.0, "a shut modal must take no room in the flow");
    assert_eq!(
        shut.1, bare.1,
        "a shut modal must draw nothing on the retained host"
    );

    assert_eq!(
        immediate_quads(Holds::SMALL, false),
        immediate_quads(Holds::Nothing, false),
        "a shut modal must draw nothing on the immediate host"
    );

    assert_both(
        &resolver,
        false,
        &[Step::Click(face)],
        &[trigger("fixture.page")],
        "press on the page",
    );
}

/// A key, or text an input method commits as a paste does, belongs to the
/// modal even while a field under it holds the keyboard: the field typed into
/// before the modal opened takes nothing more.
#[kithara::test]
fn a_standing_modal_keeps_keys_and_commits_from_the_field_under_it() {
    use iced::keyboard::key::Code;

    let resolver = Holds::SMALL.documents();
    let field = with_retained(&resolver, Page::default(), |ui| {
        centre(laid(ui, "demo/query"))
    });
    assert_both(
        &Holds::Nothing.documents(),
        false,
        &[Step::Click(field), Step::Commit("zz")],
        &[query("zz")],
        "with nothing over it the field takes the commit",
    );
    assert_both(
        &resolver,
        false,
        &[
            Step::Click(field),
            Step::Type("a", Code::KeyA),
            Step::Type("b", Code::KeyB),
            Step::Commit("zz"),
        ],
        &[query("a")],
        "the field under the modal",
    );
}

/// The cursor each host shows once the pointer arrives at a point, retained
/// first: what the retained host asked its window for, and the hand the
/// immediate tree answers with.
fn cursors(resolver: &MemResolver, open: bool, at: Pt) -> (String, String) {
    let retained = with_retained(resolver, Page::with(open), |ui| {
        pointer(ui, PointerPhase::Move, at);
        format!("{:?}", ui.take_cursor())
    });
    let ui = compiled(resolver);
    let mut host = Immediate::mount(Page::with(open), &ui, builtin::skin(), WINDOW);
    host.hover_at(at);
    (retained, format!("{:?}", host.hand()))
}

/// Hover over the scrim reaches nothing under it: above the knob and above the
/// search field each host shows the cursor it shows over a quiet part of the
/// scrim, not the one the control beneath asks for.
#[kithara::test]
fn hover_over_the_scrim_shows_nothing_of_the_controls_under_it() {
    let (face, dial) = page_points();
    let (bare, small) = (Holds::Nothing.documents(), Holds::SMALL.documents());
    let field = with_retained(&bare, Page::default(), |ui| centre(laid(ui, "demo/query")));
    let quiet = cursors(&small, true, face);
    for (name, at) in [("knob", dial), ("search field", field)] {
        let own = cursors(&bare, false, at);
        assert_ne!(
            own.0, quiet.0,
            "with nothing over it the retained {name} shows its own cursor"
        );
        assert_ne!(
            own.1, quiet.1,
            "with nothing over it the immediate {name} shows its own cursor"
        );
        assert_eq!(
            cursors(&small, true, at),
            quiet,
            "the scrim over the {name} shows what it shows anywhere else"
        );
    }
}

/// A finger on the scrim leaves the modal open as a press there does, and a
/// finger inside the content belongs to the content.
#[kithara::test]
fn a_touch_on_the_scrim_leaves_the_modal_open_on_the_immediate_host() {
    let (face, _) = page_points();
    let resolver = Holds::SMALL.documents();
    let inside = with_retained(&resolver, Page::open(), |ui| {
        centre(laid(ui, "demo/inside"))
    });
    let ui = compiled(&resolver);
    let mut host = Immediate::mount(Page::open(), &ui, builtin::skin(), WINDOW);

    host.touch_at(inside);
    assert_eq!(
        host.app().published,
        [],
        "a touch inside belongs to the content"
    );
    host.touch_at(face);
    assert_eq!(host.app().published, []);
}

/// Where a modal stands in the strip of the flow page.
#[derive(Clone, Copy, Debug)]
enum Among {
    Nowhere,
    Between,
    Last,
}

/// A row of two boxes ten apart, `{between}` and `{last}` naming where a modal
/// stands among them, and a third box right after the row: the row takes the
/// room its boxes and gaps need, so a gap the modal charged would move the
/// box after it.
const FLOW: &str = r#"Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, align: Start, children: [
    Row(id: "line", gap: 0.0, pad: 0.0, align: Start, children: [
        Row(id: "strip", size: (w: Shrink, h: Fixed(20.0)), gap: 10.0, pad: 0.0, align: Start,
            children: [
            Row(id: "a", size: (w: Fixed(20.0), h: Fixed(20.0)), children: []),
            {between}
            Row(id: "b", size: (w: Fixed(20.0), h: Fixed(20.0)), background: Danger,
                children: [Spacer(id: "b-face", size: Some((w: Fill, h: Fill)))]),
            {last}
        ]),
        Row(id: "c", size: (w: Fixed(20.0), h: Fixed(20.0)), background: Success,
            children: [Spacer(id: "c-face", size: Some((w: Fill, h: Fill)))]),
    ]),
])"#;

fn flow(among: Among) -> MemResolver {
    let modal = modal(100.0, 60.0);
    let (between, last) = match among {
        Among::Nowhere => ("", ""),
        Among::Between => (modal.as_str(), ""),
        Among::Last => ("", modal.as_str()),
    };
    page(
        "page",
        &FLOW.replace("{between}", between).replace("{last}", last),
        false,
    )
}

/// Where the danger box and the success box of a page stand on each host,
/// retained first.
fn laid_boxes(resolver: &MemResolver, open: bool) -> [[Rect; 2]; 2] {
    let retained = with_retained(resolver, Page::with(open), |ui| {
        [laid(ui, "demo/b-face"), laid(ui, "demo/c-face")]
    });
    let quads = Immediate::mount(
        Page::with(open),
        &compiled(resolver),
        builtin::skin(),
        WINDOW,
    )
    .quads();
    let filled_with = |role_: ColorRole| {
        quads
            .iter()
            .find(|(_, _, background)| *background == Background::Color(role(role_).into()))
            .map(|(_, quad, _)| Rect::from(quad.bounds))
            .unwrap_or_else(|| panic!("no box filled with {role_:?} in {quads:#?}"))
    };
    let immediate = [
        filled_with(ColorRole::Danger),
        filled_with(ColorRole::Success),
    ];
    [retained, immediate]
}

/// A modal takes no room in a flow and charges it no gap, shown or shut: the
/// boxes after it stand where they stand with no modal there at all.
#[kithara::test]
fn a_modal_takes_no_room_and_no_gap_in_a_flow() {
    let [retained, immediate] = laid_boxes(&flow(Among::Nowhere), false);
    assert_eq!(retained, immediate, "the hosts agree on the bare flow");
    for among in [Among::Between, Among::Last] {
        for open in [false, true] {
            let [shown_retained, shown_immediate] = laid_boxes(&flow(among), open);
            assert_eq!(
                shown_retained, retained,
                "the retained host, the modal {among:?}, open {open}"
            );
            assert_eq!(
                shown_immediate, immediate,
                "the immediate host, the modal {among:?}, open {open}"
            );
        }
    }
}

/// The window's resize edges answer before the modal: a press on one under
/// the scrim resizes the window and leaves the modal standing.
#[kithara::test]
fn a_resize_edge_answers_before_the_modal() {
    let edge = Pt { x: 1.0, y: 250.0 };
    let commands = [WindowCommand::Resize(WindowEdge::West)];
    let resolver = page("page", &over_page(&modal(100.0, 60.0)), true);

    with_retained(&resolver, Page::open(), |ui| {
        play_retained(ui, &[Step::Click(edge)]);
        assert_eq!(ui.take_window_commands(), commands, "the retained window");
        assert_eq!(
            ui.app().published,
            commands.map(UiEvent::Window),
            "the retained window, with the modal left standing"
        );
    });

    let compiled = compiled(&resolver);
    let mut host = Immediate::mount(Page::open(), &compiled, builtin::skin(), WINDOW);
    host.click_at(edge);
    assert_eq!(
        host.app().published,
        commands.map(UiEvent::Window),
        "the immediate window, with the modal left standing"
    );
}

/// Where the modal stands in the document against the title strip.
#[derive(Clone, Copy, Debug)]
enum Titled {
    Before,
    After,
}

/// A page whose document draws its own title strip: window controls and a
/// title bar along the top, the modal before or after them.
fn titled(order: Titled) -> MemResolver {
    let strip = r#"Row(size: (w: Fill, h: Fixed(32.0)), gap: 0.0, pad: 0.0, children: [
            WindowControls(id: "controls", style: Standard),
            TitleBar(id: "title", label: "KITHARA"),
        ]),"#;
    let modal = modal(100.0, 60.0);
    let children = match order {
        Titled::Before => format!("{modal}\n{strip}"),
        Titled::After => format!("{strip}\n{modal}"),
    };
    page("page", &over_page(&children), false)
}

/// A title bar and window controls drawn by the document lie under the scrim
/// whatever their place in it: the retained host paints the strip before the
/// scrim, a press on one reaches nothing and moves, shrinks or closes no
/// window, and hover over one shows the cursor a quiet part of the scrim shows.
#[kithara::test]
fn a_drawn_title_strip_lies_under_the_modal_whatever_its_order() {
    let title_ink = packed(role(builtin::skin().window.titlebar_text.color));
    let scrim = packed(faded(ColorRole::BgDeep, look::SCRIM_ALPHA));
    for order in [Titled::Before, Titled::After] {
        let resolver = titled(order);
        let picture = with_retained(&resolver, Page::open(), drawn);
        let title = picture.iter().rposition(|word| *word == title_ink);
        let covered = picture.iter().position(|word| *word == scrim);
        assert!(
            matches!((title, covered), (Some(title), Some(covered)) if title < covered),
            "the retained title must be painted before the scrim, modal {order:?} it: title at \
             {title:?}, scrim at {covered:?}"
        );

        let quiet = cursors(&resolver, true, Pt { x: 300.0, y: 250.0 });
        for (target, at) in [
            ("title bar", Pt { x: 300.0, y: 16.0 }),
            ("minimise cell", Pt { x: 17.5, y: 16.0 }),
            ("close cell", Pt { x: 62.5, y: 16.0 }),
        ] {
            with_retained(&resolver, Page::open(), |ui| {
                play_retained(ui, &[Step::Click(at)]);
                assert_eq!(
                    ui.take_window_commands(),
                    [],
                    "the retained {target}, modal {order:?} it"
                );
            });
            assert_both(
                &resolver,
                true,
                &[Step::Click(at)],
                &[],
                &format!("press on the {target}, modal {order:?} it"),
            );
            assert_eq!(
                cursors(&resolver, true, at),
                quiet,
                "hover over the {target} under the scrim, modal {order:?} it"
            );
        }
    }
}

/// A menu standing after the modal in the document lies under it like the
/// rest of the page: a press on the menu's row reaches nothing of the menu
/// and leaves the modal open, and hover over the row shows what the scrim shows.
#[kithara::test]
fn a_menu_after_the_modal_lies_under_it() {
    let menu = r#"Popover(id: "menu", open: Model(id: "fixture.open"), align: Start,
            anchor: Pressable(id: "burger", press: Command(id: "fixture.burger"),
                child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(20.0))))),
            content: Pressable(id: "menu-row", press: Command(id: "fixture.pick"),
                child: Spacer(id: "menu-face", size: Some((w: Fixed(100.0), h: Fixed(60.0)))))),"#;
    let resolver = page(
        "page",
        &over_page(&format!("{}\n{menu}", modal(100.0, 60.0))),
        false,
    );
    let row = with_retained(&resolver, Page::open(), |ui| {
        centre(laid(ui, "demo/menu-face"))
    });

    assert_both(&resolver, true, &[Step::Click(row)], &[], "the menu row");
    assert_eq!(
        cursors(&resolver, true, row),
        cursors(&resolver, true, Pt { x: 300.0, y: 250.0 }),
        "hover over the menu row"
    );
}

/// A stage with no size of its own holding a box, `{modal}` naming a modal
/// written before the box, and a second box after the stage: the stage takes
/// the box's room, so a stage sized by anything else would move the box
/// after it.
const STAGED: &str = r#"Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, align: Start, children: [
    Stage(id: "stage", children: [
        {modal}
        Row(id: "b", size: (w: Fixed(20.0), h: Fixed(20.0)), background: Danger,
            children: [Spacer(id: "b-face", size: Some((w: Fill, h: Fill)))]),
    ]),
    Row(id: "c", size: (w: Fixed(20.0), h: Fixed(20.0)), background: Success,
        children: [Spacer(id: "c-face", size: Some((w: Fill, h: Fill)))]),
])"#;

fn staged(modal_first: bool) -> MemResolver {
    let modal = if modal_first {
        modal(100.0, 60.0)
    } else {
        String::new()
    };
    page("page", &STAGED.replace("{modal}", &modal), false)
}

/// A stage with no size of its own takes the room of its first child in the
/// flow, so a modal written before that child changes nothing: shown or shut,
/// the box in the stage and the box after it stand where they stand with no
/// modal there, on both hosts.
#[kithara::test]
fn an_unsized_stage_takes_the_room_of_its_first_child_in_the_flow() {
    let [retained, immediate] = laid_boxes(&staged(false), false);
    assert_eq!(retained, immediate, "the hosts agree on the bare stage");
    for open in [false, true] {
        let [modal_retained, modal_immediate] = laid_boxes(&staged(true), open);
        assert_eq!(
            modal_retained, retained,
            "the retained host, the modal open {open}"
        );
        assert_eq!(
            modal_immediate, immediate,
            "the immediate host, the modal open {open}"
        );
    }
}

/// A modifier pressed while the modal stands reaches the page all the same,
/// since it types nothing: once a press on its header shuts the modal, a
/// shift-press just before the first letter of the field typed into before it
/// opened selects what it typed, and the next key replaces it.
#[kithara::test]
fn a_modifier_held_under_the_modal_reaches_the_page_once_it_shuts() {
    use iced::keyboard::key::Code;

    let resolver = Holds::Listing.documents();
    let field = with_retained(&resolver, Page::default(), |ui| laid(ui, "demo/query"));
    let header = with_retained(&resolver, Page::open(), |ui| {
        centre(laid(ui, "demo/header-face"))
    });
    let start = Pt {
        x: field.x + 40.0,
        y: field.y + field.h / 2.0,
    };
    let steps = [
        Step::Click(centre(field)),
        Step::Type("a", Code::KeyA),
        Step::Shift,
        Step::Click(header),
        Step::Click(start),
        Step::Type("b", Code::KeyB),
    ];
    let page = || Page {
        shuts: true,
        ..Page::default()
    };
    let [retained, immediate] = both(&resolver, page, &steps);
    let expected = [query("a"), trigger("fixture.shut"), query("b")];
    assert_eq!(retained, expected, "the retained host");
    assert_eq!(immediate, expected, "the immediate host");
}
