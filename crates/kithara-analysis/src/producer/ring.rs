use std::num::NonZeroU32;

use kithara_bufpool::SampleBuffer;
use num_traits::cast::ToPrimitive;
use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};
use tracing::warn;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Span {
    start: u64,
    frames: usize,
}

pub(crate) fn open_for(rate: NonZeroU32) -> (Writer, Reader) {
    const AHEAD_SECONDS: u32 = 4;
    const AHEAD_RANGES: usize = 256;

    let frames = rate.get().saturating_mul(AHEAD_SECONDS).to_usize();
    open(frames.unwrap_or(usize::MAX), AHEAD_RANGES)
}

pub(crate) fn open(frames: usize, ranges: usize) -> (Writer, Reader) {
    let (samples_tx, samples_rx) = HeapRb::<f32>::new(frames.max(1)).split();
    let (spans_tx, spans_rx) = HeapRb::<Span>::new(ranges.max(1)).split();
    (
        Writer {
            samples: samples_tx,
            spans: spans_tx,
        },
        Reader {
            samples: samples_rx,
            spans: spans_rx,
        },
    )
}

pub(crate) struct Writer {
    samples: HeapProd<f32>,
    spans: HeapProd<Span>,
}

impl Writer {
    pub(crate) fn is_open(&self) -> bool {
        self.spans.read_is_held()
    }

    pub(crate) fn push<I>(&mut self, at: u64, frames: usize, mono: I) -> bool
    where
        I: Iterator<Item = f32>,
    {
        if frames == 0 || self.samples.vacant_len() < frames || self.spans.is_full() {
            return false;
        }

        let written = self.samples.push_iter(mono.take(frames));
        if written != frames {
            warn!(
                frames,
                written, "analysis ingest: range shorter than declared; dropped"
            );
            return false;
        }
        self.spans.try_push(Span { frames, start: at }).is_ok()
    }
}

pub(crate) struct Reader {
    samples: HeapCons<f32>,
    spans: HeapCons<Span>,
}

impl Reader {
    pub(crate) fn pop(&mut self, out: &mut SampleBuffer) -> Option<u64> {
        let span = self.spans.try_pop()?;
        out.clear();
        if out.ensure_len(span.frames).is_err() {
            warn!(
                frames = span.frames,
                "analysis ingest: no room to drain a range; dropped"
            );
            self.samples.skip(span.frames);
            return None;
        }
        let read = self.samples.pop_slice(&mut out[..span.frames]);
        out.truncate(read);
        Some(span.start)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::open;
    use crate::test_pools::pools;

    #[kithara::test]
    fn a_range_comes_out_the_way_it_went_in() {
        let (mut tx, mut rx) = open(64, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        assert!(tx.push(100, 4, [1.0, 2.0, 3.0, 4.0].into_iter()));
        assert!(tx.push(200, 2, [5.0, 6.0].into_iter()));

        assert_eq!(rx.pop(&mut out), Some(100));
        assert_eq!(&out[..], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(rx.pop(&mut out), Some(200));
        assert_eq!(&out[..], &[5.0, 6.0]);
        assert_eq!(rx.pop(&mut out), None, "the ring is drained");
    }

    #[kithara::test]
    fn a_full_ring_refuses_whole_ranges() {
        let (mut tx, mut rx) = open(8, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        assert!(tx.push(0, 8, core::iter::repeat_n(1.0, 8)));
        assert!(
            !tx.push(8, 1, core::iter::once(2.0)),
            "no room left for a single frame"
        );

        assert_eq!(rx.pop(&mut out), Some(0));
        assert_eq!(out.len(), 8, "the refused range left the first one intact");
        assert_eq!(rx.pop(&mut out), None, "the refusal wrote nothing");
    }

    #[kithara::test]
    fn a_full_descriptor_ring_refuses_even_with_room_for_samples() {
        let (mut tx, mut rx) = open(64, 2);
        let pools = pools();
        let mut out = pools.get::<f32>();

        assert!(tx.push(0, 1, core::iter::once(1.0)));
        assert!(tx.push(1, 1, core::iter::once(2.0)));
        assert!(
            !tx.push(2, 1, core::iter::once(3.0)),
            "there are only two range slots"
        );

        assert_eq!(rx.pop(&mut out), Some(0));
        assert_eq!(rx.pop(&mut out), Some(1));
        assert_eq!(rx.pop(&mut out), None);
    }

    #[kithara::test]
    fn draining_frees_the_room_it_read() {
        let (mut tx, mut rx) = open(8, 4);
        let pools = pools();
        let mut out = pools.get::<f32>();

        assert!(tx.push(0, 8, core::iter::repeat_n(1.0, 8)));
        assert!(!tx.push(8, 4, core::iter::repeat_n(2.0, 4)));
        assert_eq!(rx.pop(&mut out), Some(0));
        assert!(
            tx.push(8, 4, core::iter::repeat_n(2.0, 4)),
            "a drained ring takes the range it refused"
        );

        assert_eq!(rx.pop(&mut out), Some(8));
        assert_eq!(&out[..], &[2.0, 2.0, 2.0, 2.0]);
    }
}
