use std::{
    borrow::Cow,
    sync::atomic::{AtomicUsize, Ordering},
};

use iced::{Rectangle, wgpu, widget::shader};
use wgpu::{
    BufferBindingType, BufferUsages, Color, ColorWrites, CommandEncoderDescriptor,
    DeviceDescriptor, Instance, LoadOp, MapMode, MultisampleState, PipelineCompilationOptions,
    PowerPreference, PrimitiveState, PrimitiveTopology, ShaderSource, ShaderStages, StoreOp,
    TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
};

#[derive(Debug)]
pub(super) struct VisPrimitive {
    slot: AtomicUsize,
    level: f32,
    time: f32,
    preset: u32,
}

impl VisPrimitive {
    pub(super) fn new(level: f32, preset: u32, time: f32) -> Self {
        Self {
            level,
            preset,
            time,
            slot: AtomicUsize::new(usize::MAX),
        }
    }
}

impl shader::Primitive for VisPrimitive {
    type Pipeline = VisPipeline;

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(slot) = pipeline.slots.get(self.slot.load(Ordering::Relaxed)) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.render_pipeline);
        render_pass.set_bind_group(0, &slot.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let index = pipeline.prepared;
        pipeline.prepared += 1;
        if index == pipeline.slots.len() {
            pipeline
                .slots
                .push(UniformSlot::new(device, &pipeline.bind_group_layout));
        }
        let Some(slot) = pipeline.slots.get(index) else {
            return;
        };
        let scale = viewport.scale_factor();
        let uniforms = Uniforms {
            resolution: [bounds.width * scale, bounds.height * scale],
            origin: [bounds.x * scale, bounds.y * scale],
            time: self.time,
            level: self.level,
            preset: self.preset,
        };
        queue.write_buffer(&slot.buffer, 0, &uniforms.bytes());
        self.slot.store(index, Ordering::Relaxed);
    }
}

pub(super) struct VisPipeline {
    bind_group_layout: wgpu::BindGroupLayout,
    render_pipeline: wgpu::RenderPipeline,
    slots: Vec<UniformSlot>,
    prepared: usize,
}

impl shader::Pipeline for VisPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kithara_ui.vis.bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kithara_ui.vis.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kithara_ui.vis.shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../../assets/shaders/vis.wgsl"
            ))),
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kithara_ui.vis.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Self {
            bind_group_layout,
            render_pipeline,
            prepared: 0,
            slots: Vec::new(),
        }
    }

    fn trim(&mut self) {
        self.prepared = 0;
    }
}

struct UniformSlot {
    bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

impl UniformSlot {
    const BUFFER_SIZE: u64 = Uniforms::BYTE_COUNT as u64;

    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kithara_ui.vis.uniforms"),
            size: Self::BUFFER_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            label: Some("kithara_ui.vis.bind_group"),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self { bind_group, buffer }
    }
}

#[derive(Clone, Copy)]
struct Uniforms {
    origin: [f32; 2],
    resolution: [f32; 2],
    level: f32,
    time: f32,
    preset: u32,
}

impl Uniforms {
    const BYTE_COUNT: usize = 32;

    fn bytes(self) -> [u8; Self::BYTE_COUNT] {
        let mut bytes = [0; Self::BYTE_COUNT];
        for (index, value) in [
            self.resolution[0],
            self.resolution[1],
            self.origin[0],
            self.origin[1],
            self.time,
            self.level,
        ]
        .into_iter()
        .enumerate()
        {
            let offset = index * size_of::<f32>();
            bytes[offset..offset + size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
        }
        bytes[24..28].copy_from_slice(&self.preset.to_ne_bytes());
        bytes
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use iced::wgpu::util::DeviceExt as _;
    use kithara_test_utils::kithara;

    use super::*;

    /// Render one frame of the visualiser off screen and hand back its pixels.
    ///
    /// This is the whole point of a lane with a graphics device: the uniform
    /// block is packed by hand, field by field, and nothing but a real draw
    /// tells us the shader reads back what Rust wrote.
    fn render(uniforms: Uniforms) -> Vec<u8> {
        const SIDE: u32 = 64;
        const FORMAT: wgpu::TextureFormat = TextureFormat::Rgba8Unorm;
        // A row of a mapped texture is padded to 256 bytes, and 64 pixels of
        // four bytes each is exactly that, so the readback needs no unpacking.
        const ROW_BYTES: u32 = SIDE * 4;

        let instance = Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .expect("this lane runs where a graphics device exists");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default()))
                .expect("the adapter must give up a device");

        let pipeline = <VisPipeline as shader::Pipeline>::new(&device, &queue, FORMAT);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("kithara_ui.vis.test.uniforms"),
            contents: &uniforms.bytes(),
            usage: BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kithara_ui.vis.test.bind_group"),
            layout: &pipeline.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kithara_ui.vis.test.target"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kithara_ui.vis.test.readback"),
            size: u64::from(ROW_BYTES * SIDE),
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kithara_ui.vis.test.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline.render_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW_BYTES),
                    rows_per_image: Some(SIDE),
                },
            },
            wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        readback.slice(..).map_async(MapMode::Read, |result| {
            result.expect("the readback buffer must map");
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("the queue drains");
        let pixels = readback.slice(..).get_mapped_range().to_vec();
        readback.unmap();
        pixels
    }

    fn uniforms(level: f32, preset: u32) -> Uniforms {
        Uniforms {
            origin: [0.0, 0.0],
            resolution: [64.0, 64.0],
            level,
            time: 0.5,
            preset,
        }
    }

    #[kithara::test]
    fn the_visualiser_draws_what_the_uniform_block_says() {
        let silent = render(uniforms(0.0, 0));
        let loud = render(uniforms(1.0, 0));
        let other_preset = render(uniforms(1.0, 1));

        assert_eq!(
            silent,
            render(uniforms(0.0, 0)),
            "the same uniforms must draw the same frame"
        );
        assert!(
            silent.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]),
            "the visualiser drew nothing at all"
        );
        assert_ne!(
            silent, loud,
            "the level never reached the shader: the uniform block is packed by \
             hand and its layout has drifted from assets/shaders/vis.wgsl"
        );
        assert_ne!(
            loud, other_preset,
            "the preset never reached the shader: see the layout above"
        );
    }
}
