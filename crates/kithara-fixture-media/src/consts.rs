pub(crate) static SINC: [[f32; 65]; 16] = coefficients(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/true_peak_sinc.f32le"
)));
pub(crate) static WINDOW: [[f32; 65]; 16] = coefficients(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/true_peak_window.f32le"
)));

const fn coefficients(bytes: &[u8]) -> [[f32; 65]; 16] {
    assert!(bytes.len() == 16 * 65 * size_of::<f32>());
    let mut phases = [[0.0; 65]; 16];
    let mut phase = 0;
    while phase < phases.len() {
        let mut tap = 0;
        while tap < phases[phase].len() {
            let start = (phase * 65 + tap) * size_of::<f32>();
            phases[phase][tap] = f32::from_le_bytes([
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
            ]);
            tap += 1;
        }
        phase += 1;
    }
    phases
}
