#![cfg(test)]

use std::io::{self, Read, Seek, SeekFrom};

use kithara::platform::time::Duration;
use kithara_integration_tests::memory_source::{MemorySource, memory_stream, unknown_len_stream};

#[kithara::fixture]
fn test_data() -> Vec<u8> {
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec()
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
#[case::start_0(SeekFrom::Start(0), 0, b"ABCDE")]
#[case::start_5(SeekFrom::Start(5), 5, b"FGHIJ")]
#[case::start_10(SeekFrom::Start(10), 10, b"KLMNO")]
#[case::start_20(SeekFrom::Start(20), 20, b"UVWXY")]
#[case::start_25(SeekFrom::Start(25), 25, b"Z")]
#[case::end_minus_5(SeekFrom::End(-5), 21, b"VWXYZ")]
#[case::end_minus_10(SeekFrom::End(-10), 16, b"QRSTU")]
#[case::end_minus_26(SeekFrom::End(-26), 0, b"ABCDE")]
fn seek_reads_correct_bytes(
    test_data: Vec<u8>,
    #[case] seek_from: SeekFrom,
    #[case] expected_pos: u64,
    #[case] expected: &[u8],
) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    let pos = stream.seek(seek_from).unwrap();
    assert_eq!(pos, expected_pos);

    let mut buf = vec![0u8; expected.len()];
    let n = stream.read(&mut buf).unwrap();

    assert_eq!(n, expected.len());
    assert_eq!(&buf[..n], expected);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
#[case::start_zero(SeekFrom::Start(0), 0, b"ABCDE")]
#[case::start_eof(SeekFrom::Start(26), 26, b"")]
#[case::end_zero(SeekFrom::End(0), 26, b"")]
fn seek_after_read_returns_expected_bytes(
    test_data: Vec<u8>,
    #[case] seek_from: SeekFrom,
    #[case] expected_pos: u64,
    #[case] expected: &[u8],
) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    let mut buf = [0u8; 10];
    let _ = stream.read(&mut buf).unwrap();

    let pos = stream.seek(seek_from).unwrap();
    assert_eq!(pos, expected_pos);

    let mut buf = [0u8; 5];
    let n = stream.read(&mut buf).unwrap();

    assert_eq!(n, expected.len());
    assert_eq!(&buf[..n], expected);
}

/// `SeekFrom::Current(offset)` after reading `initial_read` bytes must land
/// at `initial_read + offset` and return `expected` on the next read.
#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
#[case::forward(5, 5, 10, b"KLMNO")]
#[case::backward(10, -5, 5, b"FGHIJ")]
fn seek_current_moves_and_reads(
    test_data: Vec<u8>,
    #[case] initial_read: usize,
    #[case] offset: i64,
    #[case] expected_pos: u64,
    #[case] expected: &[u8],
) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    let mut buf = vec![0u8; initial_read];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, initial_read);

    let pos = stream.seek(SeekFrom::Current(offset)).unwrap();
    assert_eq!(pos, expected_pos);

    let mut buf = vec![0u8; expected.len()];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, expected.len());
    assert_eq!(&buf[..n], expected);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
fn seek_current_zero_stays_at_position(test_data: Vec<u8>) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    let mut buf = [0u8; 10];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 10);

    let pos = stream.stream_position().unwrap();
    assert_eq!(pos, 10);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
fn seek_end_fails_without_known_length(test_data: Vec<u8>) {
    let source = MemorySource::without_len(test_data);
    let mut stream = unknown_len_stream(source);

    let result = stream.seek(SeekFrom::End(-5));

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
#[case::past_eof_from_start(SeekFrom::Start(36))]
#[case::negative_from_current(SeekFrom::Current(-100))]
#[case::positive_offset_from_end(SeekFrom::End(10))]
fn seek_invalid_input_errors(test_data: Vec<u8>, #[case] seek_from: SeekFrom) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    let result = stream.seek(seek_from);

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
fn multiple_seeks_work_correctly(test_data: Vec<u8>) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);
    let mut results = Vec::new();

    stream.seek(SeekFrom::Start(10)).unwrap();
    let mut buf = [0u8; 1];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 1);
    results.push(buf[0]);

    stream.seek(SeekFrom::Start(5)).unwrap();
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 1);
    results.push(buf[0]);

    stream.seek(SeekFrom::Current(10)).unwrap();
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 1);
    results.push(buf[0]);

    stream.seek(SeekFrom::End(-3)).unwrap();
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 1);
    results.push(buf[0]);

    assert_eq!(results[0], b'K');
    assert_eq!(results[1], b'F');
    assert_eq!(results[2], b'Q');
    assert_eq!(results[3], b'X');
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
fn position_tracks_correctly(test_data: Vec<u8>) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);
    let mut positions = Vec::new();

    positions.push(stream.position());

    let mut buf = [0u8; 5];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 5);
    positions.push(stream.position());

    stream.seek(SeekFrom::Start(15)).unwrap();
    positions.push(stream.position());

    let mut buf = [0u8; 3];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 3);
    positions.push(stream.position());

    assert_eq!(positions[0], 0);
    assert_eq!(positions[1], 5);
    assert_eq!(positions[2], 15);
    assert_eq!(positions[3], 18);
}

#[kithara::test(timeout(Duration::from_secs(3)), hang_timeout_secs(1))]
fn seek_and_read_empty_buffer(test_data: Vec<u8>) {
    let source = MemorySource::new(test_data);
    let mut stream = memory_stream(source);

    stream.seek(SeekFrom::Start(10)).unwrap();

    let mut buf = [];
    let n = stream.read(&mut buf).unwrap();

    let pos = stream.position();

    assert_eq!(n, 0);
    assert_eq!(pos, 10);
}
