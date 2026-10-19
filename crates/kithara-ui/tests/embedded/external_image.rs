use kithara_platform::{sync::Arc, time::Duration};
use kithara_test_utils::kithara;
use kithara_ui::{
    app::{Config, Handled, Ui},
    backends::ExternalImageDeclaration,
    builtin,
    draw::{DrawListBuilder, Image, ImageId, Pt, Rect},
    interact::{
        Hit, Input, Key, Modifiers, Outcome, PointerOwnership, PointerPhase, Scroll, mouse,
    },
    registry::MapEndpoints,
    render::{
        CustomSkin, UiEvent, WindowCommand,
        custom::{CustomKinds, CustomWidget, Size2, SizeLimits, TextMeasurer},
    },
    source::MemResolver,
};
use vello::{
    Scene,
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

fn painted(ui: &mut Ui<'_, Editor>) -> Scene {
    ui.frame(Duration::from_millis(16));
    let scene = ui
        .scene()
        .expect("the preview must paint through the embedded host");
    ui.complete_frame();
    scene
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
