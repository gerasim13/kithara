use kithara::signal::sanitize_sample;

const BLOCK: u32 = 1 << 16;
const BLOCK_LEN: usize = 1 << 16;
const BLOCKS: u32 = 1 << 16;

#[kithara::test(native, flash(false))]
#[case(0)]
#[case(1)]
#[case(2)]
#[case(3)]
#[case(4)]
#[case(5)]
#[case(6)]
#[case(7)]
#[case(8)]
#[case(9)]
#[case(10)]
#[case(11)]
#[case(12)]
#[case(13)]
#[case(14)]
#[case(15)]
fn platform_sanitize_matches_signal_on_every_bit_pattern(#[case] first: u32) {
    check_blocks(first);
}

fn check_blocks(first: u32) {
    let mut samples = vec![0.0_f32; BLOCK_LEN];
    for block in (first..BLOCKS).step_by(16) {
        let base = block << 16;
        let patterns = base..=base | (BLOCK - 1);
        for (sample, bits) in samples.iter_mut().zip(patterns.clone()) {
            *sample = f32::from_bits(bits);
        }
        kithara::dsp::sanitize(&mut samples);
        for (sample, bits) in samples.iter().zip(patterns) {
            let expected = sanitize_sample(f32::from_bits(bits));
            assert_eq!(sample.to_bits(), expected.to_bits(), "pattern {bits:#010x}");
        }
    }
}
