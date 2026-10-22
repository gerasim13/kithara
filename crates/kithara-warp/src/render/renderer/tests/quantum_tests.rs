use std::num::NonZeroUsize;

use kithara_test_utils::kithara;

use super::*;
use crate::{
    Warp,
    test_pools::{TestPools, pools},
};

/// The lane packet the fixtures render: the render quantum, below the source block.
const PACKET: usize = 512;

fn unity_renderer(backend: StretchKind) -> WarpRenderer<TestPools> {
    let spec = AudioSpec::new(
        consts::CH,
        NonZeroU32::new(consts::SR).expect("sample rate"),
    );
    let config = WarpConfig::builder()
        .backend(backend)
        .speed(1.0)
        .render_quantum_frames(NonZeroUsize::new(PACKET).expect("quantum"))
        .build();
    let renderer = Warp::new((), &config).renderer(spec, pools());
    assert!(
        renderer.source_block_frames.get() >= PACKET,
        "the fixture packet is the render quantum"
    );
    renderer
}

fn chunk_meta(renderer: &WarpRenderer<TestPools>, frames: usize) -> AudioChunkInfo {
    AudioChunkInfo {
        spec: renderer.spec,
        frames: u32::try_from(frames).expect("fixture frames"),
        ..AudioChunkInfo::default()
    }
}

fn prepared_frames(backend: StretchKind, chunk: usize, output_limit: usize) -> usize {
    let mut renderer = unity_renderer(backend);
    let meta = chunk_meta(&renderer, chunk);
    renderer
        .prepare_quantum(meta, chunk, output_limit)
        .expect("unity quantum")
        .get()
}

#[kithara::test]
#[case::passthrough(StretchKind::Identity, false)]
#[case::rate(StretchKind::Glide, true)]
fn a_unity_quantum_is_one_lane_packet_whatever_the_chunk(
    #[case] backend: StretchKind,
    #[case] staging: bool,
) {
    assert_eq!(unity_renderer(backend).requires_staging(), staging);
    for chunk in [363, 661, PACKET, 4_096] {
        assert_eq!(
            prepared_frames(backend, chunk, usize::MAX),
            PACKET,
            "a {chunk}-frame decoder chunk must not size the packet"
        );
    }
}

#[kithara::test]
#[case::passthrough(StretchKind::Identity)]
#[case::rate(StretchKind::Glide)]
fn a_unity_quantum_ends_at_the_output_limit(#[case] backend: StretchKind) {
    assert_eq!(prepared_frames(backend, 661, 100), 100);
}
