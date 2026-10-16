use std::{
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use kithara_mpa::MpaReader;
use kithara_test_fixtures::mock_fixtures::{mpeg_eight, mpeg_four};
use kithara_test_utils::kithara;
use symphonia::core::{
    errors::Error as SymphoniaError,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo, SeekedTo},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    packet::Packet,
    units::Timestamp,
};

use crate::consts;

#[cfg(not(target_arch = "wasm32"))]
mod flac {
    use symphonia::{
        core::{
            codecs::{
                CodecParameters,
                audio::{AudioCodecParameters, AudioDecoderOptions},
            },
            errors::Error as SymphoniaError,
            formats::{FormatOptions, FormatReader},
            io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
            packet::Packet,
        },
        default::formats::FlacReader,
    };

    use super::{ErrorKind, InterruptingSource, SourceControl, kithara, lock};
    use crate::{
        demuxer::{DemuxOutcome, Demuxer},
        symphonia::SymphoniaDemuxer,
    };

    fn reader(bytes: &[u8], boundary: Option<usize>) -> (FlacReader<'static>, SourceControl) {
        let (source, control) = InterruptingSource::new(bytes.to_vec());
        control.make_seekable();
        assert!(source.is_seekable());
        assert_eq!(source.byte_len(), None);
        if let Some(boundary) = boundary {
            control.arm_at(boundary, usize::MAX);
        }
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        let reader =
            FlacReader::try_new(stream, FormatOptions::default()).expect("FLAC fixture reader");
        (reader, control)
    }

    fn params(reader: &FlacReader<'_>) -> AudioCodecParameters {
        let Some(CodecParameters::Audio(params)) = reader.tracks()[0].codec_params.as_ref() else {
            panic!("FLAC fixture has audio parameters");
        };
        params.clone()
    }

    fn packets(
        reader: &mut FlacReader<'_>,
        control: &SourceControl,
        boundary: Option<usize>,
    ) -> Vec<Packet> {
        let mut packets = Vec::new();
        let mut interrupted = false;
        loop {
            match reader.next_packet() {
                Ok(Some(packet)) => packets.push(packet),
                Ok(None) => break,
                Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::Interrupted => {
                    assert!(!interrupted, "the same gate must be released once");
                    assert_eq!(Some(lock(&control.state).pos), boundary);
                    interrupted = true;
                    control.release();
                }
                Err(error) => panic!("FLAC continuation failed: {error}"),
            }
        }
        assert_eq!(
            interrupted,
            boundary.is_some(),
            "fault gate must be reached"
        );
        packets
    }

    fn pcm(params: &AudioCodecParameters, packets: &[Packet]) -> Vec<u32> {
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .expect("FLAC fixture codec");
        let mut result = Vec::new();
        for packet in packets {
            let decoded = decoder.decode(packet).expect("decode FLAC packet");
            let mut samples = vec![0.0f32; decoded.samples_interleaved()];
            decoded.copy_to_slice_interleaved(&mut samples);
            result.extend(samples.into_iter().map(f32::to_bits));
        }
        result
    }

    #[kithara::test]
    fn native_continuation_after_pending_keeps_every_packet_and_pcm_sample() {
        let bytes =
            kithara_test_fixtures::assets::signal_profile_flac_flac_44100_2ch_24bit().bytes();
        let (mut reference, control) = reader(bytes, None);
        let params = params(&reference);
        let reference = packets(&mut reference, &control, None);
        let reference_pcm = pcm(&params, &reference);
        assert_eq!(reference.len(), 58, "fixed FLAC packet witness");
        assert_eq!(reference_pcm.len(), 264_600 * 2);

        for boundary in [31_743, 31_744, 31_745] {
            let (mut fault, control) = reader(bytes, Some(boundary));
            let actual = packets(&mut fault, &control, Some(boundary));
            assert_eq!(
                actual.len(),
                reference.len(),
                "boundary {boundary}; source cursor {}; actual packet clocks={:?}; expected packet clocks={:?}",
                lock(&control.state).pos,
                actual
                    .iter()
                    .map(|packet| (packet.pts.get(), packet.dur.get(), packet.data.len()))
                    .collect::<Vec<_>>(),
                reference
                    .iter()
                    .map(|packet| (packet.pts.get(), packet.dur.get(), packet.data.len()))
                    .collect::<Vec<_>>(),
            );
            for (index, (actual, expected)) in actual.iter().zip(&reference).enumerate() {
                assert_eq!(
                    actual.track_id, expected.track_id,
                    "packet {index}, boundary {boundary}"
                );
                assert_eq!(
                    actual.pts, expected.pts,
                    "packet {index}, boundary {boundary}"
                );
                assert_eq!(
                    actual.dur, expected.dur,
                    "packet {index}, boundary {boundary}"
                );
                assert!(
                    actual.data == expected.data,
                    "packet {index} bytes changed at boundary {boundary}"
                );
            }
            let actual_pcm = pcm(&params, &actual);
            assert_eq!(actual_pcm.len(), reference_pcm.len());
            assert!(
                actual_pcm == reference_pcm,
                "PCM differs at sample {:?}, boundary {boundary}",
                actual_pcm
                    .iter()
                    .zip(&reference_pcm)
                    .position(|(actual, expected)| actual != expected)
            );
            tracing::info!(
                boundary,
                packets = actual.len(),
                frames = actual_pcm.len() / 2,
                "FLAC native continuation exact"
            );
        }
    }

    #[kithara::test]
    fn demuxer_continuation_after_pending_keeps_the_whole_flac_track() {
        let bytes =
            kithara_test_fixtures::assets::signal_profile_flac_flac_44100_2ch_24bit().bytes();
        let (reference, _) = reader(bytes, None);
        let (fault, control) = reader(bytes, Some(31_744));
        let mut reference =
            SymphoniaDemuxer::from_reader_with_layout(Box::new(reference), None, None)
                .expect("reference FLAC demuxer");
        let mut fault = SymphoniaDemuxer::from_reader_with_layout(Box::new(fault), None, None)
            .expect("fault FLAC demuxer");
        let mut count = 0;
        let mut interrupted = false;
        loop {
            let actual = match fault.next_frame() {
                Ok(DemuxOutcome::Frame(frame)) => frame,
                Ok(DemuxOutcome::Pending(_)) => {
                    assert!(!interrupted, "one not-ready gate");
                    assert_eq!(lock(&control.state).pos, 31_744);
                    interrupted = true;
                    control.release();
                    continue;
                }
                Ok(DemuxOutcome::Eof) => break,
                Err(error) => {
                    let position = lock(&control.state).pos;
                    let seeks = control.seek_attempts();
                    panic!("FLAC demuxer failed at byte {position}, seeks={seeks:?}: {error}");
                }
            };
            let DemuxOutcome::Frame(expected) =
                reference.next_frame().expect("reference FLAC frame")
            else {
                panic!("reference must have the same FLAC frame");
            };
            assert_eq!(actual.pts, expected.pts, "frame {count}");
            assert_eq!(actual.duration, expected.duration, "frame {count}");
            assert!(actual.data == expected.data, "frame {count} bytes changed");
            count += 1;
        }
        assert!(interrupted, "regression must exercise not-ready recovery");
        assert!(matches!(
            reference.next_frame().expect("reference EOF"),
            DemuxOutcome::Eof
        ));
        assert_eq!(count, 58, "every FLAC packet survives Pending");
        tracing::info!(packets = count, "FLAC demuxer continuation exact");
    }
}

struct SourceState {
    interrupt_at: Option<usize>,
    interrupt_kind: ErrorKind,
    bytes: Vec<u8>,
    interrupts_remaining: usize,
    pos: usize,
    seekable: bool,
    seeks: Vec<(SeekFrom, Option<ErrorKind>)>,
}

struct SourceControl {
    state: Arc<Mutex<SourceState>>,
}

impl SourceControl {
    fn make_seekable(&self) {
        lock(&self.state).seekable = true;
    }

    fn block_after(&self, offset: usize) {
        self.arm_after(offset, usize::MAX);
        lock(&self.state).interrupt_kind = ErrorKind::WouldBlock;
    }

    fn release(&self) {
        lock(&self.state).interrupts_remaining = 0;
    }

    fn seek_attempts(&self) -> Vec<(SeekFrom, Option<ErrorKind>)> {
        lock(&self.state).seeks.clone()
    }

    fn arm_after(&self, offset: usize, count: usize) {
        let mut state = lock(&self.state);
        let interrupt_at = state.pos.saturating_add(offset);
        state.interrupt_at = Some(interrupt_at);
        state.interrupts_remaining = count;
    }

    fn arm_at(&self, pos: usize, count: usize) {
        let mut state = lock(&self.state);
        assert!(
            state.pos <= pos,
            "interruption target {pos} already passed (source at {})",
            state.pos
        );
        state.interrupt_at = Some(pos);
        state.interrupts_remaining = count;
    }
}

struct InterruptingSource {
    state: Arc<Mutex<SourceState>>,
}

impl InterruptingSource {
    const READ_CHUNK: usize = 53;

    fn new(bytes: Vec<u8>) -> (Self, SourceControl) {
        let state = Arc::new(Mutex::new(SourceState {
            bytes,
            interrupt_at: None,
            interrupt_kind: ErrorKind::Interrupted,
            interrupts_remaining: 0,
            pos: 0,
            seekable: false,
            seeks: Vec::new(),
        }));
        let control = SourceControl {
            state: Arc::clone(&state),
        };
        (Self { state }, control)
    }
}

impl Read for InterruptingSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut state = lock(&self.state);

        if state.interrupts_remaining > 0 && state.interrupt_at == Some(state.pos) {
            state.interrupts_remaining -= 1;
            return Err(io::Error::new(
                state.interrupt_kind,
                consts::INTERRUPTED_MESSAGE,
            ));
        }

        let start = state.pos;
        let mut end = start
            .saturating_add(buf.len().min(Self::READ_CHUNK))
            .min(state.bytes.len());
        if state.interrupts_remaining > 0
            && let Some(interrupt_at) = state.interrupt_at
            && start < interrupt_at
        {
            end = end.min(interrupt_at);
        }

        let read = end - start;
        buf[..read].copy_from_slice(&state.bytes[start..end]);
        state.pos = end;
        drop(state);
        Ok(read)
    }
}

impl Seek for InterruptingSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut state = lock(&self.state);
        let result = if !state.seekable {
            Err(io::Error::new(
                ErrorKind::Unsupported,
                "synthetic source is forward-only",
            ))
        } else if matches!(pos, SeekFrom::End(_)) {
            Err(io::Error::new(
                ErrorKind::Unsupported,
                "seek from end requires known length",
            ))
        } else {
            let target = match pos {
                SeekFrom::Start(target) => usize::try_from(target).ok(),
                SeekFrom::Current(offset) => state.pos.checked_add_signed(
                    isize::try_from(offset)
                        .map_err(|_| io::Error::from(ErrorKind::InvalidInput))?,
                ),
                SeekFrom::End(_) => unreachable!("unknown-length end seek handled above"),
            };
            target
                .filter(|target| *target <= state.bytes.len())
                .map_or_else(
                    || Err(io::Error::from(ErrorKind::InvalidInput)),
                    |target| {
                        state.pos = target;
                        Ok(target as u64)
                    },
                )
        };
        state
            .seeks
            .push((pos, result.as_ref().err().map(io::Error::kind)));
        result
    }
}

impl MediaSource for InterruptingSource {
    fn byte_len(&self) -> Option<u64> {
        None
    }

    fn is_seekable(&self) -> bool {
        lock(&self.state).seekable
    }
}

fn lock(state: &Arc<Mutex<SourceState>>) -> MutexGuard<'_, SourceState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn mpa_reader(source: InterruptingSource) -> MpaReader<'static> {
    let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    match MpaReader::try_new(stream, FormatOptions::default()) {
        Ok(reader) => reader,
        Err(error) => panic!("synthetic MPEG stream must open: {error}"),
    }
}

fn next_packet(reader: &mut MpaReader<'_>) -> Packet {
    match reader.next_packet() {
        Ok(Some(packet)) => packet,
        Ok(None) => panic!("synthetic MPEG stream ended early"),
        Err(error) => panic!("synthetic MPEG packet failed: {error}"),
    }
}

fn assert_packet_eq(actual: &Packet, expected: &Packet) {
    assert_eq!(actual.pts, expected.pts);
    assert_eq!(actual.dur, expected.dur);
    assert_eq!(actual.data.as_ref(), expected.data.as_ref());
}

fn assert_interrupted(reader: &mut MpaReader<'_>) {
    match reader.next_packet() {
        Err(SymphoniaError::IoError(error)) => {
            assert_eq!(error.kind(), ErrorKind::Interrupted);
            assert_eq!(error.to_string(), consts::INTERRUPTED_MESSAGE);
        }
        Ok(_) | Err(_) => panic!("packet read must return the original interruption"),
    }
}

fn seek_to(ts: i64) -> SeekTo {
    SeekTo::Timestamp {
        ts: Timestamp::new(ts),
        track_id: 0,
    }
}

fn seek_once(reader: &mut MpaReader<'_>, ts: i64) -> SeekedTo {
    match reader.seek(SeekMode::Accurate, seek_to(ts)) {
        Ok(seeked) => seeked,
        Err(error) => panic!("synthetic MPEG seek failed: {error}"),
    }
}

fn seek_after_interruption(reader: &mut MpaReader<'_>, ts: i64) -> SeekedTo {
    match reader.seek(SeekMode::Accurate, seek_to(ts)) {
        Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::Interrupted => {}
        Ok(_) => panic!("armed interruption must fire during the seek scan"),
        Err(error) => panic!("synthetic MPEG seek failed: {error}"),
    }
    for _ in 0..consts::MAX_SEEK_RETRIES {
        match reader.seek(SeekMode::Accurate, seek_to(ts)) {
            Ok(seeked) => return seeked,
            Err(SymphoniaError::IoError(error)) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => panic!("synthetic MPEG seek failed: {error}"),
        }
    }
    panic!(
        "synthetic MPEG seek did not settle after {MAX_SEEK_RETRIES} retries",
        MAX_SEEK_RETRIES = consts::MAX_SEEK_RETRIES
    )
}

fn packet_at(reader: &mut MpaReader<'_>, ts: i64) -> Packet {
    for _ in 0..consts::MAX_SEEK_RETRIES {
        let packet = next_packet(reader);
        if packet.pts.get() == ts {
            return packet;
        }
        assert!(
            packet.pts.get() < ts,
            "scan overshot pts {ts}: got {}",
            packet.pts.get()
        );
    }
    panic!("no packet reached pts {ts}")
}

#[kithara::test]
fn mpa_packet_read_rolls_back_each_interruption(mpeg_four: &'static [u8]) {
    let bytes = mpeg_four.to_vec();
    let (fault_source, fault_control) = InterruptingSource::new(bytes.clone());
    let (control_source, _) = InterruptingSource::new(bytes);
    let mut fault = mpa_reader(fault_source);
    let mut control = mpa_reader(control_source);

    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
    fault_control.arm_after(128, usize::MAX);

    assert_interrupted(&mut fault);
    assert_interrupted(&mut fault);
    fault_control.release();
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
}

#[kithara::test]
fn mpa_seek_retried_after_interruption_matches_uninterrupted_seek(mpeg_eight: &'static [u8]) {
    let bytes = mpeg_eight.to_vec();
    let (fault_source, fault_control) = InterruptingSource::new(bytes.clone());
    let (control_source, _) = InterruptingSource::new(bytes);
    let mut fault = mpa_reader(fault_source);
    let mut control = mpa_reader(control_source);

    // Interrupt mid-body of frame 4 so the seek scan is cut after it has
    // part-consumed that frame. A retried seek must resume from the same
    // frame boundary; resyncing to the next header would count the frame
    // it skipped and shift every later timestamp by one frame.
    fault_control.arm_at(4 * consts::MPEG_FRAME_LEN + consts::MPEG_FRAME_LEN / 2, 1);

    let target_ts = 6 * consts::MPEG_FRAME_DUR;
    let fault_seeked = seek_after_interruption(&mut fault, target_ts);
    let control_seeked = seek_once(&mut control, target_ts);

    assert_eq!(fault_seeked.actual_ts, control_seeked.actual_ts);
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
}

#[kithara::test]
fn mpa_seek_retry_interrupted_mid_header_matches_uninterrupted_seek(mpeg_eight: &'static [u8]) {
    let bytes = mpeg_eight.to_vec();
    let (fault_source, fault_control) = InterruptingSource::new(bytes.clone());
    let (control_source, _) = InterruptingSource::new(bytes);
    let mut fault = mpa_reader(fault_source);
    let mut control = mpa_reader(control_source);

    // Interrupt right after the first header byte of frame 4, inside
    // `sync_frame`. Without a rollback the stranded 0xFF makes the retried
    // scan resync at frame 5 while carrying frame 4's timestamp.
    fault_control.arm_at(4 * consts::MPEG_FRAME_LEN + 1, 1);

    let target_ts = 6 * consts::MPEG_FRAME_DUR;
    let fault_seeked = seek_after_interruption(&mut fault, target_ts);
    let control_seeked = seek_once(&mut control, target_ts);

    assert_eq!(fault_seeked.actual_ts, control_seeked.actual_ts);
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
}

#[kithara::test]
fn mpa_seek_retry_interrupted_in_side_info_keeps_pts_aligned_with_data(mpeg_eight: &'static [u8]) {
    let bytes = mpeg_eight.to_vec();
    let (fault_source, fault_control) = InterruptingSource::new(bytes);
    let mut fault = mpa_reader(fault_source);

    // Interrupt inside the stop frame's side info (`read_main_data_begin`).
    // A retried scan legitimately picks a shallower bit-reservoir reference
    // (its frame ring restarts at the checkpoint), so fault/control equality
    // does not hold here. The pinned property is narrower: packet data must
    // stay aligned with pts — without a rollback the retry would deliver
    // frame 7's bytes under frame 6's timestamp.
    fault_control.arm_at(6 * consts::MPEG_FRAME_LEN + 5, 1);

    let target_ts = 6 * consts::MPEG_FRAME_DUR;
    seek_after_interruption(&mut fault, target_ts);

    let packet = packet_at(&mut fault, target_ts);
    assert!(
        packet.data[4..].iter().all(|&byte| byte == 0x77),
        "packet at pts {target_ts} must carry frame 6's bytes, got fill {:#04x}",
        packet.data[4]
    );
}

#[kithara::test]
fn mpa_packets_preserve_bytes_timestamps_and_interrupted_reads(mpeg_four: &'static [u8]) {
    let (source, fault_control) = InterruptingSource::new(mpeg_four.to_vec());
    let (control_source, _) = InterruptingSource::new(mpeg_four.to_vec());
    let mut control = mpa_reader(control_source);
    let mut packets = super::packets::Packets::new(Box::new(mpa_reader(source)));
    assert!(packets.restores_interrupted_packet());
    for index in 0..4 {
        if index == 1 {
            fault_control.arm_after(128, usize::MAX);
            for _ in 0..2 {
                match packets.read() {
                    Err(SymphoniaError::IoError(error)) => {
                        assert_eq!(error.kind(), ErrorKind::Interrupted);
                    }
                    _ => panic!("packet read must return the original interruption"),
                }
            }
            fault_control.release();
        }
        let expected = next_packet(&mut control);
        let actual = packets
            .read()
            .expect("packet read")
            .expect("packet present");
        assert_eq!(actual.track_id, expected.track_id);
        assert_eq!(actual.pts, expected.pts);
        assert_eq!(actual.dur, expected.dur);
        assert_eq!(packets.data(), expected.data.as_ref());
    }
    assert!(packets.read().expect("end of stream").is_none());
}

#[kithara::test]
fn mpa_seek_preserves_packet_position(mpeg_eight: &'static [u8]) {
    let (source, _) = InterruptingSource::new(mpeg_eight.to_vec());
    let (control_source, _) = InterruptingSource::new(mpeg_eight.to_vec());
    let mut control = mpa_reader(control_source);
    let mut packets = super::packets::Packets::new(Box::new(mpa_reader(source)));
    let target = 6 * consts::MPEG_FRAME_DUR;
    let expected_seek = seek_once(&mut control, target);
    let actual_seek = packets
        .seek(SeekMode::Accurate, seek_to(target))
        .expect("pooled seek");
    assert_eq!(actual_seek.actual_ts, expected_seek.actual_ts);
    let expected = next_packet(&mut control);
    let actual = packets
        .read()
        .expect("packet read")
        .expect("packet present");
    assert_eq!(actual.pts, expected.pts);
    assert_eq!(actual.dur, expected.dur);
    assert_eq!(packets.data(), expected.data.as_ref());
}

#[cfg(not(target_arch = "wasm32"))]
mod wav {
    use kithara_platform::time::Duration;
    use kithara_test_fixtures::fixtures::tone_wav;
    use symphonia::{
        core::{
            errors::Error,
            formats::{FormatOptions, FormatReader},
            io::{MediaSourceStream, MediaSourceStreamOptions},
        },
        default::formats::WavReader,
    };

    use super::{InterruptingSource, SourceControl, kithara};
    use crate::{
        CodecPriming,
        demuxer::{DemuxOutcome, DemuxSeekOutcome, Demuxer},
        symphonia::SymphoniaDemuxer,
    };

    fn native_reader(bytes: &[u8]) -> (WavReader<'static>, SourceControl) {
        let (source, control) = InterruptingSource::new(bytes.to_vec());
        control.make_seekable();
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        let reader = WavReader::try_new(stream, FormatOptions::default()).expect("WAV reader");
        (reader, control)
    }

    fn reader(bytes: &[u8]) -> (SymphoniaDemuxer, SourceControl) {
        let (reader, control) = native_reader(bytes);
        let demuxer = SymphoniaDemuxer::from_reader_with_layout(Box::new(reader), None, None)
            .expect("WAV demuxer");
        (demuxer, control)
    }

    fn frame(demuxer: &mut SymphoniaDemuxer) -> Option<(Duration, Duration, Vec<u8>)> {
        match demuxer.next_frame().expect("WAV packet") {
            DemuxOutcome::Frame(frame) => Some((frame.pts, frame.duration, frame.data.to_vec())),
            DemuxOutcome::Eof => None,
            DemuxOutcome::Pending(reason) => {
                panic!("released WAV source is still pending: {reason:?}")
            }
        }
    }

    #[kithara::test]
    fn wav_seek_pending_preserves_every_packet_byte_and_timestamp(tone_wav: &'static [u8]) {
        let (mut fault, gate) = reader(tone_wav);
        let (mut reference, _) = reader(tone_wav);
        let target = Duration::from_millis(500);
        for demuxer in [&mut fault, &mut reference] {
            assert!(matches!(
                demuxer
                    .seek(target, CodecPriming::default())
                    .expect("WAV seek"),
                DemuxSeekOutcome::Landed { .. }
            ));
        }
        gate.block_after(128);
        match fault.next_frame().expect("interrupted WAV packet") {
            DemuxOutcome::Pending(_) => {}
            DemuxOutcome::Frame(frame) => panic!(
                "partial WAV packet was published: pts={:?} duration={:?} bytes={}",
                frame.pts,
                frame.duration,
                frame.data.len()
            ),
            DemuxOutcome::Eof => panic!("pending WAV source was reported as EOF"),
        }
        gate.release();
        let mut packets = 0;
        loop {
            let expected = frame(&mut reference);
            let actual = frame(&mut fault);
            assert_eq!(actual, expected, "WAV packet {packets} after Pending");
            if expected.is_none() {
                break;
            }
            packets += 1;
        }
        assert!(
            packets > 1,
            "seek must leave multiple WAV packets to verify"
        );
    }

    #[kithara::test]
    fn wav_truncated_eof_preserves_every_complete_frame(tone_wav: &'static [u8]) {
        let (mut complete, _) = native_reader(tone_wav);
        let declared_frames = complete.tracks()[0].num_frames.expect("WAV frame count");
        let first = complete
            .next_packet()
            .expect("WAV head")
            .expect("WAV head packet");
        let block_size = first.data.len() / usize::try_from(first.dur.get()).expect("head frames");
        assert!(block_size > 0);
        let mut expected = first.data.into_vec();
        while let Some(packet) = complete.next_packet().expect("complete WAV packet") {
            expected.extend_from_slice(&packet.data);
        }
        let removed_frames = 3;
        let removed_bytes = removed_frames * block_size;
        expected.truncate(expected.len() - removed_bytes);
        let (mut truncated, _) = native_reader(&tone_wav[..tone_wav.len() - removed_bytes]);
        let mut actual = Vec::new();
        let mut frames = 0;
        loop {
            let packet = match truncated.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(error) => panic!("truncated WAV packet: {error}"),
            };
            let packet_frames = u64::try_from(packet.data.len() / block_size).expect("PCM frames");
            assert_eq!(
                packet.dur.get(),
                packet_frames,
                "WAV duration must match actual PCM"
            );
            frames += packet_frames;
            actual.extend_from_slice(&packet.data);
        }
        assert_eq!(frames, declared_frames - removed_frames as u64);
        assert_eq!(actual, expected);
    }
}
