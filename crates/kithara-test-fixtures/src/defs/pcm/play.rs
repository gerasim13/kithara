use kithara_test_macros as kithara;

#[kithara::asset(ext = "f32le", content_type = "application/octet-stream", embed)]
#[case::half(vec![0.5_f32; 88_200])]
#[case::quarter(vec![0.25_f32; 8192])]
#[case::negative_quarter(vec![-0.25_f32; 128])]
#[case::negative_half(vec![-0.5_f32; 40])]
#[case::three_quarter(vec![0.75_f32; 8192])]
#[case::recording(vec![0.25_f32, -0.25, 0.5, -0.5])]
fn play_input(samples: Vec<f32>) -> Vec<u8> {
    samples.into_iter().flat_map(f32::to_le_bytes).collect()
}
