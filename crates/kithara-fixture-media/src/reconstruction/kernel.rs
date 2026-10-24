//! Immutable coefficients for the independent windowed-sinc true-peak reference.

use crate::consts;

/// Sixteen fractional phases, each containing 65 separate sinc and window taps.
/// Multiply a sample by its sinc tap before multiplying by its window tap.
#[must_use]
pub fn true_peak_kernel() -> (&'static [[f32; 65]; 16], &'static [[f32; 65]; 16]) {
    (&consts::SINC, &consts::WINDOW)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::true_peak_kernel;

    #[kithara::test]
    fn every_true_peak_coefficient_retains_its_host_prepared_bits_and_geometry() {
        let (sinc, window) = true_peak_kernel();
        let tables: [(_, &[u8]); 2] = [
            (
                sinc,
                include_bytes!(concat!(env!("OUT_DIR"), "/true_peak_sinc.f32le")),
            ),
            (
                window,
                include_bytes!(concat!(env!("OUT_DIR"), "/true_peak_window.f32le")),
            ),
        ];
        for (prepared, bytes) in tables {
            assert_eq!(prepared.len(), 16);
            assert!(prepared.iter().all(|phase| phase.len() == 65));
            assert_eq!(bytes.len(), 4_160);
            for (actual, expected) in prepared.iter().flatten().zip(bytes.as_chunks::<4>().0) {
                assert_eq!(actual.to_le_bytes(), *expected);
            }
        }
        assert_eq!(sinc[0][32].to_bits(), 1.0_f32.to_bits());
        assert_eq!(window[0][32].to_bits(), 1.0_f32.to_bits());
    }
}
