use std::{error::Error, path::PathBuf, thread, time::Duration};

use kithara::{
    assets::StoreOptions,
    audio::{Audio, AudioConfig, ReadOutcome},
    decode::DecoderBackend,
    file::{File, FileConfig, FileSrc},
    hls::{AbrMode, Hls, HlsConfig},
    platform::CancelToken,
    stream::Stream,
};
use url::Url;

use crate::metrics::{Baseline, Report};

const READ_BUF_SAMPLES: usize = 4096;

#[derive(Clone, Copy)]
pub(crate) enum Mode {
    Hls,
    Progressive,
    Local,
}

#[cfg(not(any(
    feature = "symphonia",
    all(feature = "apple", any(target_os = "macos", target_os = "ios"))
)))]
compile_error!("kit-bench requires `symphonia` or `apple` on macOS/iOS");

#[cfg(all(feature = "apple", any(target_os = "macos", target_os = "ios")))]
fn decoder_backend() -> DecoderBackend {
    DecoderBackend::Apple
}

#[cfg(not(all(feature = "apple", any(target_os = "macos", target_os = "ios"))))]
fn decoder_backend() -> DecoderBackend {
    DecoderBackend::Symphonia
}

pub(crate) fn run(input: &str, paced: bool, mode: Mode) -> Result<Report, Box<dyn Error>> {
    let temp = tempfile::TempDir::new()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    match mode {
        Mode::Hls => run_hls(input, paced, &temp, &rt),
        Mode::Progressive => run_file(FileSrc::Remote(Url::parse(input)?), paced, &temp, &rt),
        Mode::Local => run_file(FileSrc::Local(PathBuf::from(input)), paced, &temp, &rt),
    }
}

fn run_hls(
    url: &str,
    paced: bool,
    temp: &tempfile::TempDir,
    rt: &tokio::runtime::Runtime,
) -> Result<Report, Box<dyn Error>> {
    let parsed_url = Url::parse(url)?;
    let baseline = Baseline::take();
    let backend = decoder_backend();
    let hls_config = HlsConfig::for_url(parsed_url)
        .store(StoreOptions::new(temp.path()))
        .cancel(CancelToken::never())
        .initial_abr_mode(AbrMode::manual(0))
        .build();
    let config = AudioConfig::<Hls>::for_stream(hls_config)
        .block_on_underrun(true)
        .decoder_backend(backend)
        .build();
    let audio = rt.block_on(Audio::<Stream<Hls>>::new(config))?;

    drain_audio(audio, &baseline, backend, paced)
}

fn run_file(
    src: FileSrc,
    paced: bool,
    temp: &tempfile::TempDir,
    rt: &tokio::runtime::Runtime,
) -> Result<Report, Box<dyn Error>> {
    let baseline = Baseline::take();
    let backend = decoder_backend();
    let file_config = FileConfig::for_src(src)
        .store(StoreOptions::new(temp.path()))
        .cancel(CancelToken::never())
        .build();
    let config = AudioConfig::<File>::for_stream(file_config)
        .block_on_underrun(true)
        .decoder_backend(backend)
        .build();
    let audio = rt.block_on(Audio::<Stream<File>>::new(config))?;

    drain_audio(audio, &baseline, backend, paced)
}

fn drain_audio<S>(
    mut audio: Audio<S>,
    baseline: &Baseline,
    backend: DecoderBackend,
    paced: bool,
) -> Result<Report, Box<dyn Error>> {
    let spec = audio.spec();
    let samplerate = spec.sample_rate.get();
    let channels = spec.channels;
    let mut buf = vec![0.0f32; READ_BUF_SAMPLES];
    let mut samples = 0u64;
    let mut ttfa_ms = -1.0;

    loop {
        match audio.read(&mut buf)? {
            ReadOutcome::Pending { .. } => continue,
            ReadOutcome::Frames { count, .. } => {
                if ttfa_ms < 0.0 {
                    ttfa_ms = baseline.elapsed_ms();
                }
                samples += count.get() as u64;
                if paced {
                    let audio_pos = samples as f64 / (f64::from(samplerate) * f64::from(channels));
                    let wall = baseline.t0.elapsed().as_secs_f64();
                    if audio_pos > wall {
                        thread::sleep(Duration::from_secs_f64(audio_pos - wall));
                    }
                }
            }
            ReadOutcome::Eof { .. } => break,
        }
    }

    let report = Report::finish(
        &baseline,
        backend.to_string(),
        ttfa_ms,
        samples,
        samplerate,
        channels,
    );
    Ok(report)
}
