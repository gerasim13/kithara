use std::{f32::consts::PI, path::Path};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    let mut sinc = Vec::with_capacity(16 * 65 * 4);
    let mut window = Vec::with_capacity(16 * 65 * 4);
    for phase in 0..16_u8 {
        let offset = f32::from(phase) / 16.0;
        for tap in -32_i16..=32 {
            let distance = offset - f32::from(tap);
            let coefficient = if distance.abs() < 1e-6 {
                1.0
            } else {
                let argument = PI * distance;
                argument.sin() / argument
            };
            sinc.extend_from_slice(&coefficient.to_le_bytes());
            let coefficient = 0.5 * (1.0 + (PI * distance / 32.0).cos()).max(0.0);
            window.extend_from_slice(&coefficient.to_le_bytes());
        }
    }
    std::fs::write(output.join("true_peak_sinc.f32le"), sinc)
        .expect("write prepared sinc coefficients");
    std::fs::write(output.join("true_peak_window.f32le"), window)
        .expect("write prepared window coefficients");
    verify_original_law(&output);
}

fn verify_original_law(output: &Path) {
    const PHASES: u8 = 16;
    const HALF_WIDTH: i16 = 32;
    let sinc = std::fs::read(output.join("true_peak_sinc.f32le"))
        .expect("read prepared sinc coefficients");
    let window = std::fs::read(output.join("true_peak_window.f32le"))
        .expect("read prepared window coefficients");
    assert_eq!(sinc.len(), 16 * 65 * size_of::<f32>());
    assert_eq!(window.len(), sinc.len());
    let sinc = sinc.as_chunks::<4>().0;
    let window = window.as_chunks::<4>().0;
    let mut index = 0;
    for phase in 0..PHASES {
        let offset = f32::from(phase) / f32::from(PHASES);
        for tap in -HALF_WIDTH..=HALF_WIDTH {
            let distance = offset - f32::from(tap);
            let expected_sinc = if distance.abs() < 1e-6 {
                1.0
            } else {
                let argument = PI * distance;
                argument.sin() / argument
            };
            let expected_window =
                0.5 * (1.0 + (PI * distance / f32::from(HALF_WIDTH)).cos()).max(0.0);
            assert_eq!(
                sinc[index],
                expected_sinc.to_le_bytes(),
                "sinc {phase}/{tap}"
            );
            assert_eq!(
                window[index],
                expected_window.to_le_bytes(),
                "window {phase}/{tap}"
            );
            index += 1;
        }
    }
}
