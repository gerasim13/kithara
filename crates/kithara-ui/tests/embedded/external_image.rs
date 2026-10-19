use kithara_platform::{sync::Arc, time::Duration};
use kithara_test_utils::kithara;
use kithara_ui::{
    app::{Config, Frame, Handled, Ui},
    builtin,
    draw::{DrawListBuilder, Image, ImageId, Pt, Rect},
    interact::{
        Hit, Input, Key, Modifiers, Outcome, PointerOwnership, PointerPhase, Scroll, mouse,
    },
    registry::MapEndpoints,
    render::{
        CustomSkin, UiEvent, WindowCommand,
        custom::{CustomKinds, CustomWidget, Size2, SizeLimits, TextMeasurer},
        external::ExternalImageDeclaration,
    },
    source::MemResolver,
};
use vello::{
    Scene,
    kurbo::Affine,
    peniko::{ImageAlphaType, ImageData},
};
use vello_encoding::{DrawTag, Patch};

use super::{Editor, documents};

struct Preview {
    declaration: ExternalImageDeclaration,
    cpu: Image,
}

impl CustomWidget for Preview {
    type Action = ();

    fn external_image(&self, id: &ImageId) -> Option<&ExternalImageDeclaration> {
        (id == self.declaration.image().id()).then_some(&self.declaration)
    }

    fn measure(&mut self, _text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        Size2::new(64.0, 64.0)
    }

    fn paint(
        &mut self,
        list: &mut DrawListBuilder,
        _text: &mut TextMeasurer<'_>,
        bounds: Rect,
        _skin: &CustomSkin,
    ) {
        list.image(self.declaration.image().clone(), bounds);
        list.image(
            self.cpu.clone(),
            Rect {
                w: bounds.w / 2.0,
                ..bounds
            },
        );
    }
}

/// A preview that clips its one picture twice inside its own box and answers
/// a press, so a host that lost a clip, a turn or the press would show it.
///
/// The same widget paints either the device picture or a CPU twin with the
/// same identity and extent, which is what makes the two scenes comparable.
struct Framed {
    declaration: Option<ExternalImageDeclaration>,
    image: Image,
}

impl CustomWidget for Framed {
    type Action = ();

    fn external_image(&self, id: &ImageId) -> Option<&ExternalImageDeclaration> {
        self.declaration
            .as_ref()
            .filter(|declaration| declaration.image().id() == id)
    }

    fn input(&mut self, input: Input<'_>, hit: Hit) -> Outcome<Self::Action> {
        match input {
            Input::Pointer(pointer) if pointer.phase == PointerPhase::Down && hit.over() => {
                Outcome::set(()).with_ownership(PointerOwnership::Claim)
            }
            _ => Outcome::IGNORED,
        }
    }

    fn measure(&mut self, _text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        Size2::new(64.0, 64.0)
    }

    fn paint(
        &mut self,
        list: &mut DrawListBuilder,
        _text: &mut TextMeasurer<'_>,
        bounds: Rect,
        _skin: &CustomSkin,
    ) {
        let mut outer = list.child();
        let mut inner = outer.child();
        inner.image(self.image.clone(), bounds);
        outer.clip(inset(bounds, 4.0), inner.finish());
        list.clip(inset(bounds, 2.0), outer.finish());
    }
}

/// A widget that answers its one declaration whatever identity it is asked
/// about, and paints a second device picture under another identity with the
/// same extent. Only the host's own identity check keeps the stray picture
/// from sampling the declared texture.
struct Greedy {
    declaration: ExternalImageDeclaration,
    stray: Image,
}

impl CustomWidget for Greedy {
    type Action = ();

    fn external_image(&self, _id: &ImageId) -> Option<&ExternalImageDeclaration> {
        Some(&self.declaration)
    }

    fn measure(&mut self, _text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        Size2::new(64.0, 64.0)
    }

    fn paint(
        &mut self,
        list: &mut DrawListBuilder,
        _text: &mut TextMeasurer<'_>,
        bounds: Rect,
        _skin: &CustomSkin,
    ) {
        list.image(self.declaration.image().clone(), bounds);
        list.image(self.stray.clone(), bounds);
    }
}

/// A widget that declares one extent and paints its picture one pixel wider,
/// as a producer would after resizing its texture without declaring anew.
struct Misfit {
    declaration: ExternalImageDeclaration,
}

impl CustomWidget for Misfit {
    type Action = ();

    fn external_image(&self, id: &ImageId) -> Option<&ExternalImageDeclaration> {
        (id == self.declaration.image().id()).then_some(&self.declaration)
    }

    fn measure(&mut self, _text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        Size2::new(64.0, 64.0)
    }

    fn paint(
        &mut self,
        list: &mut DrawListBuilder,
        _text: &mut TextMeasurer<'_>,
        bounds: Rect,
        _skin: &CustomSkin,
    ) {
        let declared = self.declaration.image();
        list.image(
            Image::external(
                declared.id().clone(),
                declared.width() + 1,
                declared.height(),
            ),
            bounds,
        );
    }
}

fn inset(rect: Rect, by: f32) -> Rect {
    Rect {
        x: rect.x + by,
        y: rect.y + by,
        w: rect.w - by * 2.0,
        h: rect.h - by * 2.0,
    }
}

fn declaration() -> ExternalImageDeclaration {
    ExternalImageDeclaration::new(ImageId::new("fixture/preview"), 2, 1, ImageAlphaType::Alpha)
        .expect("the producer must declare a nonempty preview")
}

/// The CPU twin of [`declaration`]: same identity, same extent, real pixels.
fn cpu_twin(declaration: &ExternalImageDeclaration) -> Image {
    Image::pixels(
        declaration.image().id().clone(),
        2,
        1,
        Arc::from([255_u8; 8]),
    )
    .expect("two complete pixels must form a CPU picture")
}

/// Kinds holding one framed preview whose press asks the window to minimise.
fn framed(image: Image, declaration: Option<ExternalImageDeclaration>) -> CustomKinds {
    CustomKinds::default().with(
        "fixture-framed",
        move || Framed {
            declaration: declaration.clone(),
            image: image.clone(),
        },
        |()| UiEvent::Window(WindowCommand::Minimize),
    )
}

fn mounted<'a>(
    app: Editor,
    registry: &'a MapEndpoints,
    resolver: &'a MemResolver,
    kinds: &'a CustomKinds,
) -> Ui<'a, Editor> {
    Ui::new(
        app,
        Config::builder()
            .endpoints(registry)
            .resolver(resolver)
            .text(builtin::text_doc())
            .kinds(kinds)
            .build(),
        (240, 160),
        1.0,
    )
    .expect("the preview document must mount")
}

/// One frame of the embedded host, checked to declare exactly the device
/// pictures its scene samples.
fn rendered(ui: &mut Ui<'_, Editor>) -> Frame {
    ui.frame(Duration::from_millis(16));
    let frame = ui
        .render()
        .expect("the preview must paint through the embedded host");
    ui.complete_frame();
    assert_declares_what_it_samples(&frame);
    frame
}

fn painted(ui: &mut Ui<'_, Editor>) -> Scene {
    rendered(ui).into()
}

/// The tokens a frame asks its host to bind, in the order it lists them.
fn tokens(frame: &Frame) -> Vec<u64> {
    frame
        .external_images()
        .iter()
        .map(|declaration| declaration.image_data().data.id())
        .collect()
}

/// The frame declares every device picture its scene samples, once, and none
/// it does not sample. Document shaders declare their own targets apart.
fn assert_declares_what_it_samples(frame: &Frame) {
    let mut sampled: Vec<&ImageData> = Vec::new();
    for image in pictures(frame.scene()) {
        if image.data.data().is_empty() && !sampled.contains(&image) {
            sampled.push(image);
        }
    }
    let declared: Vec<&ImageData> = frame
        .external_images()
        .iter()
        .map(ExternalImageDeclaration::image_data)
        .collect();
    assert!(
        declared.iter().all(|image| sampled.contains(image)),
        "the frame declares a device picture its scene does not sample"
    );
    assert_eq!(
        declared.len() + frame.shaders().len(),
        sampled.len(),
        "the frame must declare each device picture its scene samples, once"
    );
}

/// Every picture the scene samples, in draw order.
fn pictures(scene: &Scene) -> Vec<&ImageData> {
    scene
        .encoding()
        .resources
        .patches
        .iter()
        .filter_map(|patch| {
            if let Patch::Image { image, .. } = patch {
                Some(image)
            } else {
                None
            }
        })
        .collect()
}

/// How many clips stand open around each picture, in draw order.
fn clip_depths(scene: &Scene) -> Vec<usize> {
    let mut depth = 0_usize;
    let mut depths = Vec::new();
    for &tag in &scene.encoding().draw_tags {
        if tag == DrawTag::BEGIN_CLIP {
            depth += 1;
        } else if tag == DrawTag::END_CLIP {
            depth = depth.saturating_sub(1);
        } else if tag == DrawTag::IMAGE {
            depths.push(depth);
        }
    }
    depths
}

/// What the scene draws after its last picture.
fn drawn_after_pictures(scene: &Scene) -> Vec<DrawTag> {
    let tags = &scene.encoding().draw_tags;
    let after = tags
        .iter()
        .rposition(|&tag| tag == DrawTag::IMAGE)
        .map_or(0, |last| last + 1);
    tags.iter().skip(after).copied().collect()
}

/// The device scene encodes the CPU scene's geometry, clips, transforms and
/// draw order exactly: only where the pixels come from may differ.
fn assert_same_placement(cpu: &Scene, device: &Scene) {
    let (cpu, device) = (cpu.encoding(), device.encoding());
    assert!(cpu.path_tags == device.path_tags, "path tags must match");
    assert_eq!(cpu.path_data, device.path_data);
    assert!(cpu.draw_tags == device.draw_tags, "draw tags must match");
    assert_eq!(cpu.draw_data, device.draw_data);
    assert_eq!(cpu.transforms, device.transforms);
    assert_eq!(
        [cpu.n_clips, cpu.n_open_clips],
        [device.n_clips, device.n_open_clips]
    );
    assert_eq!(device.n_open_clips, 0);
}

fn press(ui: &mut Ui<'_, Editor>, at: Pt) {
    for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
        ui.input(Input::Pointer(mouse(phase, Some(at))));
    }
}

const fn centre(rect: Rect) -> Pt {
    Pt {
        x: rect.x + rect.w / 2.0,
        y: rect.y + rect.h / 2.0,
    }
}

#[kithara::test]
fn embedded_nested_custom_image_keeps_its_token_through_action_mapping_and_repaint() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let cpu = Image::pixels(
        declaration.image().id().clone(),
        1,
        1,
        Arc::from([29_u8; 4]),
    )
    .expect("one pixel must declare a CPU picture");
    let kinds = CustomKinds::default().with(
        "fixture-preview",
        move || Preview {
            declaration: declaration.clone(),
            cpu: cpu.clone(),
        },
        |()| UiEvent::Window(WindowCommand::Minimize),
    );
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "device-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), children: [
            Row(size: (w: Fill, h: Fill), children: [
                Custom(id: "preview", kind: "fixture-preview",
                    size: Some((w: Fill, h: Fill)))
            ])
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    for _ in 0..2 {
        let scene = painted(&mut ui);
        let images = pictures(&scene);
        assert_eq!(
            images.len(),
            2,
            "both device and CPU pictures must remain in draw order"
        );
        assert_eq!(images[0].data.id(), token);
        assert!(images[0].data.data().is_empty());
        assert_eq!(images[1].data.data(), &[29_u8; 4]);
        assert_eq!([images[0].width, images[0].height], [2, 1]);
    }
}

/// A device picture inside a scrolled viewport, under a turned object and
/// inside the widget's own two clips lands exactly where its CPU twin does,
/// before and after the viewport scrolls.
#[kithara::test]
fn embedded_device_image_matches_its_cpu_twin_under_scroll_rotation_and_nested_clips() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let cpu_kinds = framed(cpu_twin(&declaration), None);
    let device_kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "turned-preview", chrome: Plain,
        root: Scroll(id: "list", size: (w: Fill, h: Fixed(120.0)),
            child: Column(gap: 0.0, pad: 0.0, children: [
                Spacer(id: "lead", size: Some((w: Fill, h: Fixed(40.0)))),
                Object(id: "spin", transform: (anchor: (32.0, 32.0), rotation: 30.0),
                    child: Custom(id: "preview", kind: "fixture-framed",
                        size: Some((w: Fixed(64.0), h: Fixed(64.0))))),
                Spacer(id: "tail", size: Some((w: Fill, h: Fixed(240.0)))),
            ])))"#,
    );
    let mut cpu = mounted(Editor::default(), &registry, &resolver, &cpu_kinds);
    let mut device = mounted(Editor::default(), &registry, &resolver, &device_kinds);
    let (cpu_still, device_still) = (painted(&mut cpu), painted(&mut device));
    assert_same_placement(&cpu_still, &device_still);

    let images = pictures(&device_still);
    assert_eq!(images.len(), 1, "the device picture must reach the scene");
    assert_eq!(images[0].data.id(), token);
    assert!(images[0].data.data().is_empty());
    assert_eq!(pictures(&cpu_still)[0].data.data(), &[255_u8; 8]);
    let depths = clip_depths(&device_still);
    assert!(
        depths.len() == 1 && depths[0] >= 3,
        "the viewport clip and both widget clips must stand open around the picture: {depths:?}"
    );
    assert!(
        device_still.encoding().transforms.iter().any(|transform| {
            let [a, b, ..] = transform.matrix;
            (b.atan2(a).to_degrees().abs() - 30.0).abs() < 0.01
        }),
        "the object's turn must reach the device picture"
    );

    let before = device
        .rect_of("demo/preview")
        .expect("the preview must be laid out");
    for ui in [&mut cpu, &mut device] {
        ui.input(Input::Pointer(mouse(
            PointerPhase::Move,
            Some(Pt { x: 200.0, y: 20.0 }),
        )));
        assert_eq!(
            ui.input(Input::Wheel(Scroll::Lines { x: 0.0, y: -1.0 })),
            Handled::Yes,
            "the viewport must take the wheel"
        );
    }
    let (cpu_scrolled, device_scrolled) = (painted(&mut cpu), painted(&mut device));
    let after = device
        .rect_of("demo/preview")
        .expect("the preview must stay laid out");
    assert!(
        after.y < before.y,
        "the wheel must scroll the preview up: {before:?} -> {after:?}"
    );
    assert_same_placement(&cpu_scrolled, &device_scrolled);
    assert_ne!(
        device_still.encoding().transforms,
        device_scrolled.encoding().transforms,
        "the scrolled picture must move with its viewport"
    );
    assert_eq!(clip_depths(&device_scrolled), depths);
    assert_eq!(
        pictures(&device_scrolled)
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token]
    );
}

/// An open modal paints its scrim over the device preview and keeps the
/// press; once it closes, the same press reaches the preview again.
#[kithara::test]
fn embedded_modal_covers_the_device_preview_and_keeps_its_press_until_closed() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "veiled-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Modal(id: "veil", open: Model(id: "fixture.flag"),
                close: Command(id: "fixture.fire"),
                content: Column(id: "surface", size: (w: Fixed(60.0), h: Fixed(40.0)),
                    gap: 0.0, pad: 0.0, children: [
                        Spacer(id: "inside", size: Some((w: Fill, h: Fill))),
                    ])),
            Custom(id: "preview", kind: "fixture-framed", size: Some((w: Fill, h: Fill))),
        ]))"#,
    );
    let mut ui = mounted(
        Editor {
            active: true,
            ..Editor::default()
        },
        &registry,
        &resolver,
        &kinds,
    );
    let veiled = painted(&mut ui);
    assert_eq!(
        pictures(&veiled)
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token],
        "the preview keeps painting its device picture under the scrim"
    );
    let over = drawn_after_pictures(&veiled);
    assert!(
        over.contains(&DrawTag::COLOR),
        "the scrim must paint after the device picture"
    );

    let at = centre(
        ui.rect_of("demo/preview")
            .expect("the preview must be laid out"),
    );
    press(&mut ui, at);
    assert!(
        ui.take_window_commands().is_empty(),
        "the scrim must keep the press from the preview"
    );
    assert!(ui.app().active, "a press on the scrim does not close it");

    ui.input(Input::KeyPressed {
        key: Key::Escape,
        modifiers: Modifiers::default(),
        text: None,
    });
    ui.input(Input::KeyReleased {
        key: Key::Escape,
        modifiers: Modifiers::default(),
    });
    assert!(!ui.app().active, "Escape must write the modal's close");
    let open = painted(&mut ui);
    assert!(
        drawn_after_pictures(&open).len() < over.len(),
        "the closed modal must stop painting over the preview"
    );
    assert_eq!(
        pictures(&open)
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token]
    );
    press(&mut ui, at);
    assert!(
        matches!(
            ui.take_window_commands().as_slice(),
            [WindowCommand::Minimize]
        ),
        "the uncovered preview must take the press"
    );
}

/// An open popover paints its surface over the device preview and keeps the
/// press that lands on it. A press beside it is the dismissal, which the
/// preview does not see either; once the popover has shut, the same press
/// reaches the preview.
#[kithara::test]
fn embedded_popover_covers_the_device_preview_and_keeps_its_presses_until_dismissed() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "menu-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Popover(id: "menu", open: View(id: "menu"), align: Start,
                anchor: Pressable(id: "burger", press: View(id: "menu"),
                    child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(20.0))))),
                content: Spacer(id: "inside", size: Some((w: Fixed(100.0), h: Fixed(60.0))))),
            Custom(id: "preview", kind: "fixture-framed", size: Some((w: Fill, h: Fill))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    let shut = drawn_after_pictures(&painted(&mut ui));
    let anchor = ui
        .rect_of("demo/anchor")
        .expect("the anchor the popover hangs on must be laid out");
    press(&mut ui, centre(anchor));
    let open = painted(&mut ui);
    assert_eq!(
        pictures(&open)
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token],
        "the preview keeps painting its device picture under the surface"
    );
    let over = drawn_after_pictures(&open);
    assert!(
        over.len() > shut.len() && over.contains(&DrawTag::COLOR),
        "the open surface must paint after the device picture: {} -> {} draws",
        shut.len(),
        over.len()
    );

    let preview = ui
        .rect_of("demo/preview")
        .expect("the preview must be laid out");
    let surface = ui
        .rect_of("demo/inside")
        .expect("the open popover must lay its surface out");
    let covered = Pt {
        x: surface.x + surface.w / 2.0,
        y: surface.y.max(preview.y) + 4.0,
    };
    assert!(
        covered.y < surface.y + surface.h && covered.y < preview.y + preview.h,
        "the surface must hang over the preview: {surface:?} over {preview:?}"
    );
    press(&mut ui, covered);
    assert!(
        ui.take_window_commands().is_empty(),
        "the surface must keep the press from the preview under it"
    );
    assert!(
        drawn_after_pictures(&painted(&mut ui)) == over,
        "a press on the surface leaves it open"
    );

    let beside = Pt {
        x: (surface.x + surface.w).midpoint(preview.x + preview.w),
        y: covered.y,
    };
    press(&mut ui, beside);
    assert!(
        ui.take_window_commands().is_empty(),
        "the press that dismisses the popover must not reach the preview"
    );
    assert!(
        drawn_after_pictures(&painted(&mut ui)) == shut,
        "the dismissed popover must stop painting over the preview"
    );
    press(&mut ui, beside);
    assert!(
        matches!(
            ui.take_window_commands().as_slice(),
            [WindowCommand::Minimize]
        ),
        "the uncovered preview must take the press"
    );
}

/// The host binds a declaration only to the picture that names it. A widget
/// that answers the same declaration whatever identity it is asked about
/// still gets the token on its own picture alone; the stray picture is
/// refused rather than drawn with another producer's texture.
#[kithara::test]
fn embedded_host_binds_a_declaration_only_to_the_picture_it_names() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let stray = Image::external(ImageId::new("fixture/stray"), 2, 1);
    let kinds = CustomKinds::default().with(
        "fixture-greedy",
        move || Greedy {
            declaration: declaration.clone(),
            stray: stray.clone(),
        },
        |()| UiEvent::Window(WindowCommand::Minimize),
    );
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "greedy-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Custom(id: "preview", kind: "fixture-greedy", size: Some((w: Fill, h: Fill))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    let frame = rendered(&mut ui);
    assert_eq!(
        pictures(frame.scene())
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token],
        "exactly one picture may sample the declared token"
    );
    assert_eq!(tokens(&frame), [token]);
}

/// A picture painted at an extent other than the one its declaration names is
/// refused, and the frame does not ask the host to bind a token its scene
/// never samples.
#[kithara::test]
fn embedded_frame_does_not_declare_a_picture_refused_for_its_extent() {
    let declaration = declaration();
    let kinds = CustomKinds::default().with(
        "fixture-misfit",
        move || Misfit {
            declaration: declaration.clone(),
        },
        |()| UiEvent::Window(WindowCommand::Minimize),
    );
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "misfit-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Custom(id: "preview", kind: "fixture-misfit", size: Some((w: Fill, h: Fill))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    let frame = rendered(&mut ui);
    assert!(
        pictures(frame.scene()).is_empty(),
        "a picture at another extent than its declaration's must be refused"
    );
    assert!(
        tokens(&frame).is_empty(),
        "a refused picture declares nothing"
    );
}

/// A frame declares a device picture only while its widget paints into the
/// scene. Behind a hidden `Optional` the widget is stashed: its picture leaves
/// the scene and its declaration leaves the frame, and both come back with it.
#[kithara::test]
fn embedded_frame_declares_a_device_picture_only_while_it_is_shown() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "optional-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Chip(id: "fire", label: "TOGGLE", size: Some((w: Fill, h: Fixed(40.0))),
                read: Model(id: "fixture.flag"), write: Command(id: "fixture.fire")),
            Optional(id: "panel", hidden: Model(id: "fixture.hidden"),
                child: Custom(id: "preview", kind: "fixture-framed",
                    size: Some((w: Fill, h: Fill)))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    assert!(
        tokens(&rendered(&mut ui)).is_empty(),
        "a hidden preview declares nothing"
    );
    let toggle = centre(
        ui.rect_of("demo/fire")
            .expect("the toggle must be laid out"),
    );
    for shown in [true, false, true] {
        press(&mut ui, toggle);
        assert_eq!(ui.app().active, shown);
        let expected = if shown { vec![token] } else { Vec::new() };
        assert_eq!(
            tokens(&rendered(&mut ui)),
            expected,
            "the frame must follow the preview being shown: {shown}"
        );
    }
}

/// A preview inside a popover's surface is declared while the popover stands
/// open, and not once a press beside the surface has shut it.
#[kithara::test]
fn embedded_frame_drops_the_device_picture_of_a_shut_popover() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "menu-preview", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Popover(id: "menu", open: View(id: "menu"), align: Start,
                anchor: Pressable(id: "burger", press: View(id: "menu"),
                    child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(20.0))))),
                content: Custom(id: "preview", kind: "fixture-framed",
                    size: Some((w: Fixed(64.0), h: Fixed(64.0))))),
            Spacer(id: "room", size: Some((w: Fill, h: Fill))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    assert!(
        tokens(&rendered(&mut ui)).is_empty(),
        "a shut popover's preview declares nothing"
    );
    let anchor = ui
        .rect_of("demo/anchor")
        .expect("the anchor the popover hangs on must be laid out");
    press(&mut ui, centre(anchor));
    assert_eq!(tokens(&rendered(&mut ui)), [token]);

    press(&mut ui, Pt { x: 230.0, y: 150.0 });
    assert!(
        tokens(&rendered(&mut ui)).is_empty(),
        "the dismissed popover's preview must leave the frame"
    );
}

/// Two widgets painting one producer's picture sample the same token, and the
/// frame asks the host to bind it once.
#[kithara::test]
fn embedded_frame_declares_a_shared_device_picture_once() {
    let declaration = declaration();
    let token = declaration.image_data().data.id();
    let kinds = framed(declaration.image().clone(), Some(declaration));
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "twin-previews", chrome: Plain,
        root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Custom(id: "left", kind: "fixture-framed", size: Some((w: Fixed(64.0), h: Fixed(64.0)))),
            Custom(id: "right", kind: "fixture-framed", size: Some((w: Fixed(64.0), h: Fixed(64.0)))),
        ]))"#,
    );
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    let frame = rendered(&mut ui);
    assert_eq!(
        pictures(frame.scene())
            .iter()
            .map(|image| image.data.id())
            .collect::<Vec<_>>(),
        [token, token],
        "both previews must draw the shared picture"
    );
    assert_eq!(tokens(&frame), [token]);
}

const FIELD: &str = r"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(position.xy / kithara.viewport.xy, 0.0, 1.0);
}
";

/// A document shader paints the target its pass renders into as one picture
/// with no pixels, as large as its box and placed on it. That picture is the
/// shader's to declare, not a custom widget's.
#[kithara::test]
fn embedded_shader_picture_is_placed_on_its_box() {
    let kinds = CustomKinds::default();
    let (registry, mut resolver) = documents();
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "shaded", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Spacer(id: "lead", size: Some((w: Fill, h: Fixed(12.0)))),
            Shader(id: "field", source: "field.wgsl",
                uniforms: { "flag": Model(id: "fixture.flag") },
                size: (w: Fixed(30.0), h: Fixed(20.0))),
        ]))"#,
    );
    resolver.insert("field.wgsl", FIELD);
    let mut ui = mounted(Editor::default(), &registry, &resolver, &kinds);
    let frame = rendered(&mut ui);
    assert_eq!(frame.shaders().len(), 1);
    assert!(frame.external_images().is_empty());
    let images = pictures(frame.scene());
    assert_eq!(images.len(), 1, "the shader must paint one picture");
    assert!(images[0].data.data().is_empty());
    let field = ui
        .rect_of("demo/field")
        .expect("the shader must be laid out");
    let extent = [images[0].width, images[0].height];
    assert_eq!(extent, [30, 20], "the target must be as large as {field:?}");
    let expected = Affine::translate((f64::from(field.x), f64::from(field.y)))
        * Affine::scale_non_uniform(
            f64::from(field.w) / f64::from(extent[0]),
            f64::from(field.h) / f64::from(extent[1]),
        );
    let [a, b, c, d, e, f] = expected.as_coeffs();
    assert!(
        frame.scene().encoding().transforms.iter().any(|transform| {
            let [m0, m1, m2, m3] = transform.matrix;
            let [t0, t1] = transform.translation;
            [m0, m1, m2, m3, t0, t1]
                .iter()
                .zip([a, b, c, d, e, f])
                .all(|(got, want)| (f64::from(*got) - want).abs() < 1e-4)
        }),
        "the shader picture must be placed on {field:?}"
    );
}
