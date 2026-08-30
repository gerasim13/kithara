use futures_lite::future::block_on;
use masonry::vello::{
    AaConfig, AaSupport, RenderParams, Renderer, RendererOptions,
    peniko::Color,
    wgpu::{
        Backends, Device, DeviceDescriptor, Extent3d, Instance, InstanceDescriptor, Queue,
        RequestAdapterOptions, Texture, TextureDescriptor, TextureDimension, TextureFormat,
        TextureUsages, TextureViewDescriptor,
    },
};

use crate::{
    app::Frame,
    backends::read_back,
    render::{shader::ShaderPass, vis::VisPass},
};

/// A wgpu device with no surface, plus the Vello renderer that targets it.
///
/// This is what photographs the retained host: it draws the same scene a
/// window would and reads the pixels back, so a headless machine can compare
/// the two hosts page by page.
pub struct Offscreen {
    device: Device,
    queue: Queue,
    renderer: Renderer,
    shaders: ShaderPass,
    texture: Texture,
    vis: VisPass,
    height: u32,
    width: u32,
}

impl Offscreen {
    /// # Errors
    /// Fails when the machine offers no wgpu adapter or device, or when Vello
    /// cannot build its renderer.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::PRIMARY,
            ..InstanceDescriptor::default()
        });
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
            label: Some("kithara-capture"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::STORAGE_BINDING
                | TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let shaders = ShaderPass::new(&device);
        let vis = VisPass::new(&device, TextureFormat::Rgba8Unorm);
        Ok(Self {
            device,
            height,
            queue,
            renderer,
            shaders,
            texture,
            vis,
            width,
        })
    }

    /// Rasterises one scene into `out` as tightly packed RGBA.
    ///
    /// The caller owns the pixels so a walk over a set of pages fills one
    /// buffer per set rather than one per page.
    ///
    /// The base colour is the window's, not this capture's: the page behind a
    /// document belongs to the skin, and a set cleared to anything else
    /// differs from the other host wherever a document leaves its rectangle
    /// bare.
    ///
    /// # Errors
    /// Fails when the scene cannot be rendered or the pixels cannot be read
    /// back.
    pub fn rasterise(
        &mut self,
        frame: &Frame,
        scale: f64,
        base: Color,
        out: &mut Vec<u8>,
    ) -> Result<(), String> {
        let view = self.texture.create_view(&TextureViewDescriptor::default());
        self.shaders.render(
            &self.device,
            &self.queue,
            &mut self.renderer,
            frame.shaders(),
        );
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                frame.scene(),
                &view,
                &RenderParams {
                    base_color: base,
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| format!("render_to_texture: {error}"))?;
        self.vis.render(
            &self.device,
            &self.queue,
            &view,
            frame.vis(),
            scale,
            [self.width, self.height],
        );

        read_back(
            &self.device,
            &self.queue,
            &self.texture,
            (self.width, self.height),
            out,
        )
    }
}
