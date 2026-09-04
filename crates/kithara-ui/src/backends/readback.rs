use vello::wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Device, Extent3d, MapMode, PollType,
    Queue, TexelCopyBufferInfo, TexelCopyBufferLayout, Texture,
};

/// Copies a rendered texture back into `out` as tightly packed RGBA8.
///
/// wgpu starts every copied row on a 256-byte boundary, so the rows are
/// unpadded on the way out. The caller owns the destination: a walk over a set
/// of pages fills the same buffer once per page instead of allocating one per
/// page.
///
/// # Errors
/// Fails when the device cannot be polled to completion.
pub(crate) fn read_back(
    device: &Device,
    queue: &Queue,
    texture: &Texture,
    size: (u32, u32),
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let (width, height) = size;
    let unpadded = width * 4;
    let padded = unpadded.div_ceil(256) * 256;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("kithara-readback"),
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
    out.clear();
    out.reserve((unpadded * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        out.extend_from_slice(&mapped[start..start + unpadded as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(())
}
