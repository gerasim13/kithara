use std::{
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use kithara_test_utils::kithara;
use symphonia::{
    core::{
        errors::Error as SymphoniaError,
        formats::{FormatOptions, FormatReader},
        io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
        packet::Packet,
    },
    default::formats::MpaReader,
};

const INTERRUPTED_MESSAGE: &str = "synthetic MPEG frame interruption";
const MPEG_FRAME_LEN: usize = 417;

struct SourceState {
    bytes: Vec<u8>,
    interrupt_at: Option<usize>,
    interrupts_remaining: usize,
    pos: usize,
}

struct SourceControl {
    state: Arc<Mutex<SourceState>>,
}

impl SourceControl {
    fn arm_after(&self, offset: usize, count: usize) {
        let mut state = lock(&self.state);
        let interrupt_at = state.pos.saturating_add(offset);
        state.interrupt_at = Some(interrupt_at);
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
            interrupts_remaining: 0,
            pos: 0,
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
            return Err(io::Error::new(ErrorKind::Interrupted, INTERRUPTED_MESSAGE));
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
        Ok(read)
    }
}

impl Seek for InterruptingSource {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "synthetic source is forward-only",
        ))
    }
}

impl MediaSource for InterruptingSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn lock(state: &Arc<Mutex<SourceState>>) -> MutexGuard<'_, SourceState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn mpeg_frames() -> Vec<u8> {
    let mut frames = Vec::with_capacity(4 * MPEG_FRAME_LEN);
    for fill in [0x11, 0x22, 0x33, 0x44] {
        let mut frame = [fill; MPEG_FRAME_LEN];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        frames.extend_from_slice(&frame);
    }
    frames
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
            assert_eq!(error.to_string(), INTERRUPTED_MESSAGE);
        }
        Ok(_) | Err(_) => panic!("packet read must return the original interruption"),
    }
}

#[kithara::test]
fn mpa_packet_read_rolls_back_each_interruption() {
    let bytes = mpeg_frames();
    let (fault_source, fault_control) = InterruptingSource::new(bytes.clone());
    let (control_source, _) = InterruptingSource::new(bytes);
    let mut fault = mpa_reader(fault_source);
    let mut control = mpa_reader(control_source);

    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
    fault_control.arm_after(128, 2);

    assert_interrupted(&mut fault);
    assert_interrupted(&mut fault);
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
    assert_packet_eq(&next_packet(&mut fault), &next_packet(&mut control));
}
