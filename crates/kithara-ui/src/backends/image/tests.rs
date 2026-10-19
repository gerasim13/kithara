use kithara_platform::sync::Arc;
use kithara_test_utils::kithara;
use vello::{Scene, peniko::ImageAlphaType};
use vello_encoding::Patch;

use super::VelloImageBackend;
use crate::{
    backends::{ExternalImageDeclaration, VelloBackend},
    draw::{Backend, DrawListBuilder, Image, ImageId, Rect, Rgba, Transform, replay},
};

const BOUNDS: Rect = Rect {
    h: 20.0,
    w: 40.0,
    x: 13.0,
    y: 29.0,
};

fn declaration() -> ExternalImageDeclaration {
    ExternalImageDeclaration::new(
        ImageId::new("test/device-picture"),
        2,
        1,
        ImageAlphaType::Alpha,
    )
    .expect("a named positive extent must declare a device picture")
}

#[kithara::test]
fn external_image_declaration_keeps_one_stable_token_and_explicit_alpha() {
    let declaration = ExternalImageDeclaration::new(
        ImageId::new("test/alpha-picture"),
        2,
        1,
        ImageAlphaType::AlphaPremultiplied,
    )
    .expect("a named positive extent must declare a premultiplied picture");
    let cloned = declaration.clone();

    assert_eq!(declaration.image(), cloned.image());
    assert!(declaration.image().rgba().is_none());
    assert!(declaration.image_data().data.data().is_empty());
    assert_eq!(
        declaration.image_data().data.id(),
        cloned.image_data().data.id()
    );
    assert_eq!(
        declaration.image_data().alpha_type,
        ImageAlphaType::AlphaPremultiplied
    );
    assert_eq!(
        [
            declaration.image_data().width,
            declaration.image_data().height
        ],
        [declaration.image().width(), declaration.image().height()]
    );
}

#[kithara::test]
fn external_image_declaration_rejects_blank_identity_and_empty_extent() {
    for (id, width, height) in [("", 2, 1), (" \t", 2, 1), ("valid", 0, 1), ("valid", 2, 0)] {
        assert!(
            ExternalImageDeclaration::new(ImageId::new(id), width, height, ImageAlphaType::Alpha)
                .is_none()
        );
    }
}

#[kithara::test]
fn external_image_replay_matches_cpu_rotation_and_nested_clip_order() {
    let declaration = declaration();
    let cpu = Image::pixels(
        declaration.image().id().clone(),
        2,
        1,
        Arc::from([255_u8; 8]),
    )
    .expect("two complete pixels must form a CPU picture");
    let list = |image| {
        let mut list = DrawListBuilder::default();
        list.transformed(Transform::rotate(std::f32::consts::FRAC_PI_4), |list| {
            let mut outer = list.child();
            let mut inner = outer.child();
            inner.fill_rect(
                BOUNDS,
                Rgba {
                    a: 1.0,
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                },
            );
            inner.image(image, BOUNDS);
            inner.fill_rect(
                BOUNDS,
                Rgba {
                    a: 1.0,
                    r: 0.0,
                    g: 1.0,
                    b: 0.0,
                },
            );
            outer.clip(BOUNDS, inner.finish());
            list.clip(BOUNDS, outer.finish());
        });
        list.finish()
    };
    let mut cpu_scene = Scene::new();
    replay(&list(cpu), &mut VelloBackend::new(&mut cpu_scene));
    let mut device_scene = Scene::new();
    replay(
        &list(declaration.image().clone()),
        &mut VelloImageBackend::new(&mut device_scene, |id: &ImageId| {
            (id == declaration.image().id()).then_some(declaration.image_data())
        }),
    );

    let cpu = cpu_scene.encoding();
    let device = device_scene.encoding();
    assert_eq!(device.n_open_clips, 0);
    assert!(device.n_clips >= 4);
    assert!(cpu.path_tags == device.path_tags, "path tags must match");
    assert_eq!(cpu.path_data, device.path_data);
    assert!(cpu.draw_tags == device.draw_tags, "draw tags must match");
    assert_eq!(cpu.draw_data, device.draw_data);
    assert_eq!(cpu.transforms, device.transforms);
    let tokens: Vec<_> = device
        .resources
        .patches
        .iter()
        .filter_map(|patch| {
            if let Patch::Image { image, .. } = patch {
                Some(image.data.id())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(tokens, [declaration.image_data().data.id()]);
}

#[kithara::test]
fn external_image_lookup_does_not_override_cpu_pixels_with_the_same_identity() {
    let declaration = declaration();
    let cpu = Image::pixels(
        declaration.image().id().clone(),
        1,
        1,
        Arc::from([17_u8; 4]),
    )
    .expect("one complete pixel must form a CPU picture");
    let mut scene = Scene::new();
    VelloImageBackend::new(
        &mut scene,
        |_id: &ImageId| -> Option<&vello::peniko::ImageData> {
            panic!("CPU pixels must never consult a device binding")
        },
    )
    .image(&cpu, BOUNDS, 0.0);
    let Some(Patch::Image { image, .. }) = scene.encoding().resources.patches.first() else {
        panic!("the CPU picture must reach the scene");
    };
    assert_eq!(image.data.data(), &[17_u8; 4]);
    assert_ne!(image.data.id(), declaration.image_data().data.id());
}

#[kithara::test]
fn external_image_replay_rejects_missing_and_inconsistent_bindings() {
    let declaration = declaration();
    let mut missing = Scene::new();
    VelloImageBackend::new(&mut missing, |_id: &ImageId| None).image(
        declaration.image(),
        BOUNDS,
        0.0,
    );
    assert!(missing.encoding().resources.patches.is_empty());

    for (width, height) in [(0, 1), (2, 0), (3, 1), (2, 3)] {
        let image = Image::external(declaration.image().id().clone(), width, height);
        let mut scene = Scene::new();
        VelloImageBackend::new(&mut scene, |_id: &ImageId| Some(declaration.image_data()))
            .image(&image, BOUNDS, 0.0);
        assert!(scene.encoding().resources.patches.is_empty());
    }
}
