use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    ops::ControlFlow,
    path::Path,
};

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct ReadSummary {
    pub(super) invalid_utf8_lines: usize,
    pub(super) oversized_lines: usize,
    pub(super) stopped_early: bool,
    pub(super) record_limit_exceeded: bool,
}

pub(super) fn for_each_bounded_line(
    path: &Path,
    max_line_bytes: usize,
    callback: impl FnMut(&str) -> ControlFlow<()>,
) -> io::Result<ReadSummary> {
    for_each_bounded_line_with_limit(path, max_line_bytes, usize::MAX, callback)
}

pub(super) fn for_each_bounded_line_with_limit(
    path: &Path,
    max_line_bytes: usize,
    max_records: usize,
    mut callback: impl FnMut(&str) -> ControlFlow<()>,
) -> io::Result<ReadSummary> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::with_capacity(max_line_bytes.min(8 * 1_024));
    let mut discarding = false;
    let mut summary = ReadSummary::default();
    let mut records = 0usize;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if discarding {
                if !accept_record(&mut records, max_records, &mut summary) {
                    summary.stopped_early = true;
                    return Ok(summary);
                }
                summary.oversized_lines = summary.oversized_lines.saturating_add(1);
            } else if !line.is_empty()
                && (!accept_record(&mut records, max_records, &mut summary)
                    || emit_line(&mut line, &mut callback, &mut summary).is_break())
            {
                summary.stopped_early = true;
            }
            return Ok(summary);
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let segment_end = newline.unwrap_or(available.len());
        if !discarding {
            if line.len().saturating_add(segment_end) > max_line_bytes {
                line.clear();
                discarding = true;
            } else {
                line.extend_from_slice(&available[..segment_end]);
            }
        }
        reader.consume(consumed);

        if newline.is_some() {
            if !accept_record(&mut records, max_records, &mut summary) {
                summary.stopped_early = true;
                return Ok(summary);
            }
            if discarding {
                summary.oversized_lines = summary.oversized_lines.saturating_add(1);
                discarding = false;
            } else if emit_line(&mut line, &mut callback, &mut summary).is_break() {
                summary.stopped_early = true;
                return Ok(summary);
            }
        }
    }
}

fn accept_record(records: &mut usize, max_records: usize, summary: &mut ReadSummary) -> bool {
    if *records >= max_records {
        summary.record_limit_exceeded = true;
        return false;
    }
    *records = records.saturating_add(1);
    true
}

fn emit_line(
    line: &mut Vec<u8>,
    callback: &mut impl FnMut(&str) -> ControlFlow<()>,
    summary: &mut ReadSummary,
) -> ControlFlow<()> {
    let bytes = if line.last() == Some(&b'\r') {
        &line[..line.len() - 1]
    } else {
        line.as_slice()
    };
    let result = match std::str::from_utf8(bytes) {
        Ok(text) => callback(text),
        Err(_) => {
            summary.invalid_utf8_lines = summary.invalid_utf8_lines.saturating_add(1);
            ControlFlow::Continue(())
        }
    };
    line.clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_lines_and_bounds_individual_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("records.log");
        std::fs::write(&path, b"first\nway-too-long\nbad:\xff\nlast\r\n").expect("write fixture");
        let mut lines = Vec::new();

        let summary = for_each_bounded_line(&path, 8, |line| {
            lines.push(line.to_owned());
            ControlFlow::Continue(())
        })
        .expect("read fixture");

        assert_eq!(lines, ["first", "last"]);
        assert_eq!(summary.oversized_lines, 1);
        assert_eq!(summary.invalid_utf8_lines, 1);
        assert!(!summary.stopped_early);
    }

    #[test]
    fn callback_can_stop_without_reading_the_remainder() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("records.log");
        std::fs::write(&path, "first\nsecond\n").expect("write fixture");

        let summary =
            for_each_bounded_line(&path, 64, |_| ControlFlow::Break(())).expect("read fixture");

        assert!(summary.stopped_early);
        assert!(!summary.record_limit_exceeded);
    }

    #[test]
    fn record_limit_stops_before_dispatching_excess_input() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("records.log");
        std::fs::write(&path, "first\nsecond\n").expect("write fixture");
        let mut lines = Vec::new();

        let summary = for_each_bounded_line_with_limit(&path, 64, 1, |line| {
            lines.push(line.to_owned());
            ControlFlow::Continue(())
        })
        .expect("read fixture");

        assert_eq!(lines, ["first"]);
        assert!(summary.stopped_early);
        assert!(summary.record_limit_exceeded);
    }
}
