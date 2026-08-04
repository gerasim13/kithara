//! Headless Masonry capture: the same gallery documents, drawn by the Masonry
//! host into a Vello scene, rasterised off-screen and written as PNG.
//!
//! Enabled by `KITHARA_GALLERY_CAPTURE_MASONRY=<dir>`; it runs instead of the
//! iced window so the two hosts can be compared page by page.

use std::{env, fs::create_dir_all, path::PathBuf};

use futures_lite::future::block_on;
use kithara_ui::{
    builtin,
    compile::compile,
    render::{
        document,
        masonry::{MasonryHost, MasonryRoot, MasonryState},
    },
    source::UiConfig,
};
use masonry::{
    app::{RenderRootOptions, WindowSizePolicy},
    core::WindowEvent,
    dpi::PhysicalSize,
    theme::default_property_set,
    vello::{
        AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene,
        peniko::color::palette,
        wgpu::{
            Backends, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Device,
            DeviceDescriptor, Extent3d, Instance, InstanceDescriptor, MapMode, PollType, Queue,
            RequestAdapterOptions, TexelCopyBufferInfo, TexelCopyBufferLayout, Texture,
            TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
            TextureViewDescriptor,
        },
    },
};
use num_traits::cast::AsPrimitive;

use super::{
    Consts,
    capture::{Shot, write_png},
    mock, resolver,
};

/// A wgpu device with no surface, plus the Vello renderer that targets it.
struct Offscreen {
    device: Device,
    queue: Queue,
    renderer: Renderer,
    texture: Texture,
    width: u32,
    height: u32,
}

impl Offscreen {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let instance = Instance::new(&wgpu_options());
        let adapter = block_on(instance.request_adapter(&RequestAdapterOptions::default()))
            .map_err(|error| format!("no wgpu adapter: {error}"))?;
        let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor::default()))
            .map_err(|error| format!("no wgpu device: {error}"))?;
        let renderer = Renderer::new(
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
            label: Some("gallery-masonry"),
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
        Ok(Self {
            device,
            queue,
            renderer,
            texture,
            width,
            height,
        })
    }

    /// Rasterises one scene and reads the pixels back as tightly packed RGBA.
    fn rasterise(&mut self, scene: &Scene) -> Result<Vec<u8>, String> {
        let view = self.texture.create_view(&TextureViewDescriptor::default());
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &view,
                &RenderParams {
                    base_color: palette::css::BLACK,
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| format!("render_to_texture: {error}"))?;

        // wgpu requires each copied row to start on a 256-byte boundary.
        let unpadded = self.width * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("gallery-masonry-readback"),
            size: u64::from(padded) * u64::from(self.height),
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(MapMode::Read, |_| {});
        self.device
            .poll(PollType::Wait)
            .map_err(|error| format!("poll: {error}"))?;
        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded * self.height) as usize);
        for row in 0..self.height {
            let start = (row * padded) as usize;
            rgba.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(rgba)
    }
}

fn wgpu_options() -> InstanceDescriptor {
    InstanceDescriptor {
        backends: Backends::PRIMARY,
        ..Default::default()
    }
}

/// Walks every gallery page through the Masonry host. Returns `false` when the
/// environment variable is absent, so the caller falls through to the window.
pub(super) fn run() -> bool {
    let Some(dir) = env::var_os("KITHARA_GALLERY_CAPTURE_MASONRY").map(PathBuf::from) else {
        return false;
    };
    match capture(&dir) {
        Ok(count) => println!("{count} page(s) written to {}", dir.display()),
        Err(error) => eprintln!("masonry capture failed: {error}"),
    }
    true
}

fn capture(dir: &PathBuf) -> Result<usize, String> {
    create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let width = AsPrimitive::<u32>::as_(Consts::WIDTH);
    let height = AsPrimitive::<u32>::as_(Consts::HEIGHT);
    let mut off = Offscreen::new(width, height)?;
    let resolver = resolver();
    let endpoints = mock::registry();
    let skin = builtin::skin();
    let mut written = 0;

    for shot in Shot::all() {
        let mut reads = mock::MockReads::default();
        reads.select_tab(shot.tab);
        if let Some(module) = shot.module {
            reads.select_module(module);
        }
        let entry = shot.module.map_or_else(|| shot.tab.entry(), |m| m.entry());
        let ui = compile(
            entry,
            &resolver,
            &endpoints,
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .map_err(|error| format!("compile {entry}: {error}"))?;

        let host = MasonryHost::new(&ui, &reads, skin).with_state(MasonryState::default());
        let node = document::render(&ui.root, &ui, &reads, builtin::skin_doc(), host);
        let mut root = MasonryRoot::new(
            node,
            RenderRootOptions {
                default_properties: kithara_platform::sync::Arc::new(default_property_set()),
                use_system_fonts: false,
                size_policy: WindowSizePolicy::User,
                size: PhysicalSize::new(width, height),
                scale_factor: 1.0,
                test_font: None,
            },
        )
        .map_err(|error| format!("masonry root: {error}"))?;
        root.handle_window_event(WindowEvent::Resize(PhysicalSize::new(width, height)))
            .map_err(|error| format!("resize: {error}"))?;
        let (scene, _) = root.redraw().map_err(|error| format!("redraw: {error}"))?;
        let rgba = off.rasterise(&scene)?;
        let path = dir.join(format!("{}.png", shot.name()));
        write_png(&path, &rgba, width, height)?;
        println!("captured {}", path.display());
        written += 1;
    }
    Ok(written)
}
