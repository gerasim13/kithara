use kithara_test_macros as kithara;

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::ring(vec![0.1, 0.2, 0.7, 0.8])]
#[case::planar(vec![1.0, 2.0, f32::NAN, 3.0, 4.0, f32::NAN])]
fn mock_pcm(samples: Vec<f32>) -> Vec<u8> {
    samples.into_iter().flat_map(f32::to_le_bytes).collect()
}

#[kithara::asset(ext = "bin", content_type = "application/octet-stream", embed)]
#[case::ramp((1i16..=8).flat_map(i16::to_le_bytes).collect())]
#[case::zero(vec![0; 4])]
#[case::one(vec![1; 4])]
fn mock_packet(bytes: Vec<u8>) -> Vec<u8> {
    bytes
}

#[kithara::asset(ext = "mp3", content_type = "audio/mpeg", embed)]
#[case::four(&[0x11, 0x22, 0x33, 0x44])]
#[case::eight(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88])]
fn mpeg_frames(fills: &[u8]) -> Vec<u8> {
    let mut frames = Vec::with_capacity(fills.len() * 417);
    for &fill in fills {
        let mut frame = [fill; 417];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        frames.extend_from_slice(&frame);
    }
    frames
}
