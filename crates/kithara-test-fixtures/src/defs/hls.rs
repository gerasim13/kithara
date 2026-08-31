use std::{borrow::Cow, io, sync::OnceLock};

use aes::Aes128;
use cbc::{
    Encryptor,
    cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7},
};
use kithara_bufpool::{BytePool, SamplePool};
use kithara_encode::{EncoderFactory, PackagedEncodeRequest};
use kithara_stream::{AudioCodec, ContainerFormat, MediaInfo};
use kithara_test_macros as kithara;
use rayon::prelude::*;

use crate::{
    context::BuildContext,
    fmp4::{Fmp4Package, GaplessEncoding, mux_audio_track},
    hls_manifest::{Manifest, Resource},
    signal::{Pcm, Wave},
};

struct Consts;

impl Consts {
    const CHANNELS: u16 = 2;
    const ENCODERS: usize = 4;
    const IV: [u8; 16] = [0; 16];
    const KEY: [u8; 16] = *b"0123456789abcdef";
    const SAMPLE_RATE: u32 = 44_100;
    const SEGMENT_FRAMES: usize = 264_600;
    const SEGMENTS: usize = 37;
    const TARGET_DURATION: u8 = 7;
    const VARIANTS: [VariantSpec; 4] = [
        VariantSpec {
            bandwidth: 66_005,
            bit_rate: 64_000,
            codec: AudioCodec::AacLc,
            codecs: "mp4a.40.2",
            label: "slq",
        },
        VariantSpec {
            bandwidth: 134_107,
            bit_rate: 128_000,
            codec: AudioCodec::AacLc,
            codecs: "mp4a.40.2",
            label: "smq",
        },
        VariantSpec {
            bandwidth: 269_930,
            bit_rate: 256_000,
            codec: AudioCodec::AacLc,
            codecs: "mp4a.40.2",
            label: "shq",
        },
        VariantSpec {
            bandwidth: 988_758,
            bit_rate: 512_000,
            codec: AudioCodec::Flac,
            codecs: "fLaC",
            label: "slossless",
        },
    ];
}

#[derive(Clone, Copy)]
struct VariantSpec {
    bandwidth: u64,
    bit_rate: u64,
    codec: AudioCodec,
    codecs: &'static str,
    label: &'static str,
}

struct Variant {
    package: Fmp4Package,
    spec: VariantSpec,
}

fn encode(spec: VariantSpec) -> Variant {
    let frame_samples = EncoderFactory::frame_samples(spec.codec).unwrap_or_else(|error| {
        panic!(
            "kithara-test-fixtures: {:?} frame size failed: {error}",
            spec.codec
        )
    });
    let packets_per_segment = Consts::SEGMENT_FRAMES.div_ceil(frame_samples);
    let encoded_frames = packets_per_segment
        .checked_mul(frame_samples)
        .and_then(|frames| frames.checked_mul(Consts::SEGMENTS))
        .expect("invariant: long HLS fixture frame count fits usize");
    // FFmpeg emits one native AAC priming access unit before the source frames.
    let total_frames = if spec.codec == AudioCodec::AacLc {
        encoded_frames
            .checked_sub(frame_samples)
            .expect("invariant: AAC fixture includes native priming")
    } else {
        encoded_frames
    };
    let pcm = Pcm::new(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        total_frames,
        Wave::Sawtooth,
    );
    let media_info = MediaInfo::builder()
        .codec(spec.codec)
        .container(ContainerFormat::Fmp4)
        .sample_rate(Consts::SAMPLE_RATE)
        .channels(Consts::CHANNELS)
        .build();
    let track = EncoderFactory::encode_packaged(
        &PackagedEncodeRequest::for_pools(BytePool::default(), SamplePool::default())
            .pcm(&pcm)
            .media_info(media_info)
            .timescale(Consts::SAMPLE_RATE)
            .bit_rate(spec.bit_rate)
            .packets_per_segment(packets_per_segment)
            .encoder_delay(0)
            .trailing_delay(0)
            .build(),
    )
    .unwrap_or_else(|error| {
        panic!(
            "kithara-test-fixtures: {:?} long HLS encode failed: {error}",
            spec.codec
        )
    });
    let package = mux_audio_track(&track, GaplessEncoding::None).unwrap_or_else(|error| {
        panic!(
            "kithara-test-fixtures: {:?} long HLS mux failed: {error}",
            spec.codec
        )
    });
    Variant { package, spec }
}

fn variants() -> &'static [Variant] {
    static ENCODED: OnceLock<Vec<Variant>> = OnceLock::new();
    ENCODED.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(Consts::ENCODERS)
            .build()
            .expect("invariant: fixture encoder pool builds")
            .install(|| Consts::VARIANTS.par_iter().copied().map(encode).collect())
    })
}

fn encrypt(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len() + 16);
    output.extend_from_slice(bytes);
    output.resize(bytes.len() + 16, 0);
    Encryptor::<Aes128>::new((&Consts::KEY).into(), (&Consts::IV).into())
        .encrypt_padded::<Pkcs7>(&mut output, bytes.len())
        .expect("invariant: one padding block is reserved")
        .to_vec()
}

fn body(bytes: &[u8], encrypted: bool) -> Cow<'_, [u8]> {
    if encrypted {
        Cow::Owned(encrypt(bytes))
    } else {
        Cow::Borrowed(bytes)
    }
}

fn add(
    context: &BuildContext<'_>,
    resources: &mut Vec<Resource>,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .ok_or_else(|| io::Error::other(format!("HLS resource `{name}` has no extension")))?;
    resources.push(Resource {
        content_type: content_type.to_owned(),
        file: context.store(name, ext, bytes)?,
        route: format!("/hls/{name}"),
    });
    Ok(())
}

fn media_playlist(variant: &Variant, encrypted: bool) -> String {
    let label = variant.spec.label;
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:{}\n#EXT-X-ALLOW-CACHE:YES\n\
         #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-VERSION:6\n#EXT-X-MEDIA-SEQUENCE:1\n",
        Consts::TARGET_DURATION
    );
    if encrypted {
        playlist.push_str(&format!(
            "#EXT-X-KEY:METHOD=AES-128,URI={label}.key,IV=0x{}\n",
            hex::encode_upper(Consts::IV)
        ));
    }
    playlist.push_str(&format!("#EXT-X-MAP:URI=\"init-{label}-a1.mp4\"\n"));
    for (index, duration) in variant.package.segment_durations_secs.iter().enumerate() {
        playlist.push_str(&format!(
            "#EXTINF:{duration:.3},\nsegment-{}-{label}-a1.m4s\n",
            index + 1
        ));
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    playlist
}

fn master_playlist() -> String {
    let mut master = String::from("#EXTM3U\n");
    for spec in Consts::VARIANTS {
        master.push_str(&format!(
            "#EXT-X-STREAM-INF:PROGRAM-ID=1,BANDWIDTH={},CODECS=\"{}\",AVERAGE-BANDWIDTH={}\n\
             index-{}-a1.m3u8\n",
            spec.bandwidth, spec.codecs, spec.bandwidth, spec.label
        ));
    }
    master
}

fn bundle(context: &BuildContext<'_>, encrypted: bool) -> io::Result<Vec<u8>> {
    let mut resources = Vec::new();
    for variant in variants() {
        let label = variant.spec.label;
        if encrypted {
            add(
                context,
                &mut resources,
                &format!("{label}.key"),
                "application/octet-stream",
                &Consts::KEY,
            )?;
        }
        add(
            context,
            &mut resources,
            &format!("init-{label}-a1.mp4"),
            "audio/mp4",
            &body(&variant.package.init_segment, encrypted),
        )?;
        for (index, segment) in variant.package.media_segments.iter().enumerate() {
            add(
                context,
                &mut resources,
                &format!("segment-{}-{label}-a1.m4s", index + 1),
                "audio/mp4",
                &body(segment, encrypted),
            )?;
        }
        add(
            context,
            &mut resources,
            &format!("index-{label}-a1.m3u8"),
            "application/vnd.apple.mpegurl",
            media_playlist(variant, encrypted).as_bytes(),
        )?;
    }
    add(
        context,
        &mut resources,
        "master.m3u8",
        "application/vnd.apple.mpegurl",
        master_playlist().as_bytes(),
    )?;
    resources.sort_by(|left, right| left.route.cmp(&right.route));
    toml::to_string(&Manifest {
        master: "/hls/master.m3u8".to_owned(),
        resources,
    })
    .map(String::into_bytes)
    .map_err(io::Error::other)
}

#[kithara::asset(
    ext = "toml",
    content_type = "application/x-kithara-hls-bundle",
    context
)]
#[case::plain(false)]
#[case::drm(true)]
fn long_hls(context: &BuildContext<'_>, encrypted: bool) -> Vec<u8> {
    bundle(context, encrypted)
        .unwrap_or_else(|error| panic!("kithara-test-fixtures: long HLS bundle failed: {error}"))
}
