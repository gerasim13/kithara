//! One clipped list, both backends, one question: does what a clip contains
//! reach the pixels?
//!
//! Every other test in the crate asks a backend what it *recorded* — that a
//! clip is in the list, that the scene balanced its layers. Both answered yes
//! while the production renderer painted nothing inside the clip, because the
//! headless harness rasterises through a different renderer than the
//! application does. This module rasterises instead, through the renderer the
//! application actually uses, and looks at a pixel.

use futures_lite::future::block_on;
use iced::{
    Color, Pixels, Size, Vector,
    advanced::{
        Renderer as _,
        graphics::{geometry::Renderer as _, text::font_system},
        renderer::Headless,
    },
    widget::canvas::Frame,
};
use kithara_test_utils::kithara;
use vello::{
    AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene,
    kurbo::Affine,
    peniko::color::palette,
    wgpu::{
        BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Device, DeviceDescriptor,
        Extent3d, Instance, InstanceDescriptor, MapMode, PollType, Queue, RequestAdapterOptions,
        TexelCopyBufferInfo, TexelCopyBufferLayout, TextureDescriptor, TextureDimension,
        TextureFormat, TextureUsages, TextureViewDescriptor,
    },
};

use super::{VelloBackend, replay_ordered};
use crate::{
    builtin,
    draw::{DrawList, DrawListBuilder, Rect, Rgba, replay},
    render::fonts::{FONT_BYTES, SANS},
};

/// The one fixture both backends are asked to paint.
struct Fixture;

impl Fixture {
    /// The surface they paint into.
    const SURFACE: (u32, u32) = (96, 96);

    /// Where the control sits on it. A widget almost never starts at the
    /// window's origin, and the defect this module exists for only appears when
    /// it does not.
    const ORIGIN: (f32, f32) = (24.0, 24.0);

    /// The clip, and a rectangle wholly inside it.
    const REGION: Rect = Rect {
        h: 40.0,
        w: 40.0,
        x: 12.0,
        y: 12.0,
    };

    const INSIDE: Rect = Rect {
        h: 20.0,
        w: 20.0,
        x: 22.0,
        y: 22.0,
    };

    /// What the clip contains: white, so every channel is high.
    const INK: Rgba = Rgba {
        a: 1.0,
        b: 1.0,
        g: 1.0,
        r: 1.0,
    };

    /// What is drawn under it, outside any clip: red, so a single channel tells
    /// which of the two won.
    const UNDER: Rgba = Rgba {
        a: 1.0,
        b: 0.0,
        g: 0.0,
        r: 1.0,
    };
}

/// A control that fills its own box and then draws inside a clip — the order
/// every real control uses, and the one a backend has to keep.
fn clipped() -> DrawList {
    let mut list = DrawListBuilder::default();
    list.fill_rect(Fixture::REGION, Fixture::UNDER);
    let mut inner = DrawListBuilder::default();
    inner.fill_rect(Fixture::INSIDE, Fixture::INK);
    list.clip(Fixture::REGION, inner.finish());
    list.finish()
}

/// Whether the clip's contents won the centre pixel. The rectangle under it is
/// red and the clipped one is white, so a high green channel means the clipped
/// rectangle is on top — which is the order the list asked for.
fn clip_is_on_top(rgba: &[u8]) -> bool {
    let x = (Fixture::ORIGIN.0 + Fixture::INSIDE.x + Fixture::INSIDE.w / 2.0) as usize;
    let y = (Fixture::ORIGIN.1 + Fixture::INSIDE.y + Fixture::INSIDE.h / 2.0) as usize;
    let pixel = (y * Fixture::SURFACE.0 as usize + x) * 4;
    rgba.get(pixel + 1).is_some_and(|green| *green > 128)
}

#[kithara::test]
fn vello_paints_what_a_clip_contains() {
    let mut control = Scene::new();
    replay(&clipped(), &mut VelloBackend::new(&mut control));
    let mut scene = Scene::new();
    scene.append(
        &control,
        Some(Affine::translate((
            f64::from(Fixture::ORIGIN.0),
            f64::from(Fixture::ORIGIN.1),
        ))),
    );
    let rgba = rasterise(&scene).unwrap_or_else(|error| panic!("vello must rasterise: {error}"));

    assert!(
        clip_is_on_top(&rgba),
        "the Vello backend painted its own geometry over what the clip contained"
    );
}

#[kithara::test]
fn iced_paints_what_a_clip_contains() {
    let skin = builtin::skin();
    let mut fonts = font_system()
        .write()
        .unwrap_or_else(|error| panic!("the iced font system must be available: {error}"));
    for bytes in FONT_BYTES {
        fonts.load_font(std::borrow::Cow::Borrowed(bytes));
    }
    drop(fonts);

    let mut renderer = block_on(<iced::Renderer as Headless>::new(
        SANS,
        Pixels(14.0),
        Some("wgpu"),
    ))
    .unwrap_or_else(|| panic!("iced must give a wgpu renderer without a window"));
    let mut frame = Frame::new(
        &renderer,
        Size::new(Fixture::SURFACE.0 as f32, Fixture::SURFACE.1 as f32),
    );
    replay_ordered(&clipped(), &mut frame, skin.text_resources());
    let geometry = frame.into_geometry();
    renderer.with_translation(
        Vector::new(Fixture::ORIGIN.0, Fixture::ORIGIN.1),
        |renderer| {
            renderer.draw_geometry(geometry);
        },
    );
    let rgba = renderer.screenshot(
        Size::new(Fixture::SURFACE.0, Fixture::SURFACE.1),
        1.0,
        Color::from_rgb(0.0, 0.0, 0.0),
    );

    assert!(
        clip_is_on_top(&rgba),
        "the iced backend painted its own geometry over what the clip contained"
    );
}

/// A wgpu device with no surface, and one scene rasterised through it.
fn rasterise(scene: &Scene) -> Result<Vec<u8>, String> {
    let (width, height) = Fixture::SURFACE;
    let instance = Instance::new(&InstanceDescriptor::default());
    let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default()))
        .map_err(|error| format!("no wgpu adapter: {error}"))?;
    let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor::default()))
        .map_err(|error| format!("no wgpu device: {error}"))?;
    let mut renderer = Renderer::new(
        &device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .map_err(|error| format!("vello renderer: {error}"))?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("clip-conformance"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer
        .render_to_texture(
            &device,
            &queue,
            scene,
            &texture.create_view(&TextureViewDescriptor::default()),
            &RenderParams {
                base_color: palette::css::BLACK,
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        )
        .map_err(|error| format!("render_to_texture: {error}"))?;
    read_back(&device, &queue, &texture, (width, height))
}

/// Copies the target texture back, undoing wgpu's 256-byte row padding.
fn read_back(
    device: &Device,
    queue: &Queue,
    texture: &vello::wgpu::Texture,
    size: (u32, u32),
) -> Result<Vec<u8>, String> {
    let (width, height) = size;
    let unpadded = width * 4;
    let padded = unpadded.div_ceil(256) * 256;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("clip-conformance-readback"),
        size: u64::from(padded) * u64::from(height),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    slice.map_async(MapMode::Read, |_| {});
    device
        .poll(PollType::Wait)
        .map_err(|error| format!("poll: {error}"))?;
    let mapped = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        rgba.extend_from_slice(&mapped[start..start + unpadded as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(rgba)
}
