use kithara_platform::sync::Arc;
use kithara_test_utils::kithara;
use vello::{Scene, peniko::ImageAlphaType};
use vello_encoding::{Encoding, Patch};

use super::VelloImageBackend;
use crate::{
    backends::VelloBackend,
    draw::{Backend, DrawListBuilder, Image, ImageId, Pen, Pt, Rect, Rgba, Transform, replay},
    masonry::external::ExternalImageDeclaration,
};

const BOUNDS: Rect = Rect {
    h: 20.0,
    w: 40.0,
    x: 13.0,
    y: 29.0,
};

const RED: Rgba = Rgba {
    a: 1.0,
    r: 1.0,
    g: 0.0,
    b: 0.0,
};

const GREEN: Rgba = Rgba {
    a: 1.0,
    r: 0.0,
    g: 1.0,
    b: 0.0,
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

/// The two encodings place the same paths, clips, transforms and styles in the
/// same draw order. Where a picture's pixels come from is left to the caller.
fn assert_same_geometry(left: &Encoding, right: &Encoding) {
    assert!(left.path_tags == right.path_tags, "path tags must match");
    assert_eq!(left.path_data, right.path_data);
    assert!(left.draw_tags == right.draw_tags, "draw tags must match");
    assert_eq!(left.draw_data, right.draw_data);
    assert_eq!(left.transforms, right.transforms);
    assert!(left.styles == right.styles, "styles must match");
    assert_eq!(
        [
            left.n_paths,
            left.n_path_segments,
            left.n_clips,
            left.n_open_clips
        ],
        [
            right.n_paths,
            right.n_path_segments,
            right.n_clips,
            right.n_open_clips
        ]
    );
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
    assert_eq!(
        declaration,
        declaration.clone(),
        "a clone binds the same token"
    );
    assert_ne!(
        declaration,
        ExternalImageDeclaration::new(
            ImageId::new("test/alpha-picture"),
            2,
            1,
            ImageAlphaType::AlphaPremultiplied,
        )
        .expect("the same picture must declare again"),
        "declaring the same picture again mints a new token for the host to bind"
    );

    let data = declaration.image_data();
    assert!(declaration.image().rgba().is_none() && data.data.data().is_empty());
    assert_eq!(data.alpha_type, ImageAlphaType::AlphaPremultiplied);
    assert_eq!(
        [data.width, data.height],
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

/// A replay that meets no device picture encodes exactly what the plain
/// backend does, so routing a custom leaf through the lookup changes nothing
/// for a widget that declares none.
#[kithara::test]
fn image_backend_without_device_pictures_encodes_what_the_plain_backend_does() {
    let cpu = Image::pixels(
        ImageId::new("test/cpu-picture"),
        1,
        1,
        Arc::from([17_u8; 4]),
    )
    .expect("one complete pixel must form a CPU picture");
    let mut list = DrawListBuilder::default();
    list.fill_rect(BOUNDS, RED);
    list.stroke_line(
        Pt { x: 0.0, y: 0.0 },
        Pt { x: 40.0, y: 30.0 },
        GREEN,
        Pen::new(2.0),
    );
    let mut outer = list.child();
    let mut inner = outer.child();
    inner.image(cpu, BOUNDS);
    inner.fill_rect(BOUNDS, GREEN);
    outer.clip(BOUNDS, inner.finish());
    list.clip(BOUNDS, outer.finish());
    let list = list.finish();

    let mut plain = Scene::new();
    replay(&list, &mut VelloBackend::new(&mut plain));
    let mut routed = Scene::new();
    replay(
        &list,
        &mut VelloImageBackend::new(&mut routed, |_id: &ImageId| None),
    );

    let (plain, routed) = (plain.encoding(), routed.encoding());
    assert!(plain.n_clips >= 2);
    assert_same_geometry(plain, routed);
    let pictures = |encoding: &Encoding| {
        encoding
            .resources
            .patches
            .iter()
            .map(|patch| {
                if let Patch::Image {
                    image,
                    draw_data_offset,
                } = patch
                {
                    Some((
                        *draw_data_offset,
                        image.data.data().to_vec(),
                        image.format,
                        image.alpha_type,
                        [image.width, image.height],
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let routed = pictures(routed);
    assert_eq!(pictures(plain), routed);
    assert!(
        matches!(routed.as_slice(), [Some((_, pixels, ..))] if pixels.as_slice() == [17_u8; 4]),
        "the CPU picture must reach the scene once, with its own pixels"
    );
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
            inner.fill_rect(BOUNDS, RED);
            inner.image(image, BOUNDS);
            inner.fill_rect(BOUNDS, GREEN);
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

    let device = device_scene.encoding();
    assert_eq!(device.n_open_clips, 0);
    assert!(device.n_clips >= 4);
    assert_same_geometry(cpu_scene.encoding(), device);
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
    let [Patch::Image { image, .. }] = scene.encoding().resources.patches.as_slice() else {
        panic!("the CPU picture must reach the scene as exactly one picture");
    };
    assert_eq!(image.data.data(), &[17_u8; 4]);
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
