use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::{SyncApplied, SyncExecutionReject, SyncExecutionStamp, SyncReceipt, consts};

/// The sole audio-thread writer for one allocated slot's execution receipts.
pub struct SyncReceiptTx(HeapProd<SyncReceipt>);

/// The owner's reader of one allocated slot's execution receipts, with the
/// one receipt a failed owner update handed back for the next drain.
pub struct SyncReceiptInbox {
    rx: HeapCons<SyncReceipt>,
    kept: Option<SyncReceipt>,
}

/// Two slots held for one activation until its first PCM span is consumed.
/// Only the audio claim it is handed to can write them.
#[must_use]
pub(crate) struct ReceiptReservation<'a> {
    tx: &'a mut SyncReceiptTx,
    pair: [SyncReceipt; consts::RECEIPT_PAIR],
}

/// Make the per-slot, single-producer receipt channel off the audio thread.
#[must_use]
pub fn sync_receipts() -> (SyncReceiptTx, SyncReceiptInbox) {
    let (tx, rx) = HeapRb::<SyncReceipt>::new(consts::RECEIPT_PAIR).split();
    (SyncReceiptTx(tx), SyncReceiptInbox { rx, kept: None })
}

impl SyncReceiptTx {
    /// Reserve `Armed` and `Presented` of `applied` before the audio gate can
    /// be claimed, or `None` while a receipt of this slot is still waiting.
    #[inline]
    pub(crate) fn reserve_pair(&mut self, applied: SyncApplied) -> Option<ReceiptReservation<'_>> {
        (self.0.vacant_len() >= consts::RECEIPT_PAIR).then_some(ReceiptReservation {
            tx: self,
            pair: [
                SyncReceipt::Armed(applied.stamp()),
                SyncReceipt::Presented(applied),
            ],
        })
    }

    /// Record a pre-claim rejection into one bounded slot.
    ///
    /// # Errors
    ///
    /// Returns the rejection when the mailbox has no vacant slot for it.
    #[inline]
    pub(crate) fn publish_rejected(
        &mut self,
        stamp: SyncExecutionStamp,
        reason: SyncExecutionReject,
    ) -> Result<(), SyncReceipt> {
        self.0.try_push(SyncReceipt::Rejected { stamp, reason })
    }
}

impl ReceiptReservation<'_> {
    /// The stamp of the preparation both reserved receipts report.
    pub(crate) const fn stamp(&self) -> SyncExecutionStamp {
        self.pair[1].stamp()
    }

    /// Write the reserved pair after actual nonempty PCM consumption. The
    /// reservation holds the sole producer since it saw both slots vacant,
    /// and the owner can only free slots.
    ///
    /// # Panics
    ///
    /// Panics if the reserved pair no longer fits in the sole producer's ring.
    pub(crate) fn publish(self) {
        let written = self.tx.0.push_slice(&self.pair);
        assert_eq!(
            written,
            consts::RECEIPT_PAIR,
            "reserved sync receipts must fit"
        );
    }
}

impl SyncReceiptInbox {
    /// The next receipt in callback order: the one a failed owner update
    /// kept is the head of the queue, then the ring.
    #[must_use]
    pub fn next_receipt(&mut self) -> Option<SyncReceipt> {
        if self.kept.is_some() {
            return self.kept.take();
        }
        self.rx.try_pop()
    }

    /// Hand back the receipt [`Self::next_receipt`] just returned, so the
    /// next drain reads it first.
    pub fn keep(&mut self, receipt: SyncReceipt) {
        debug_assert!(self.kept.is_none(), "one receipt is kept at a time");
        self.kept = Some(receipt);
    }

    /// True once the sole audio-thread producer has been dropped.
    ///
    /// The owner uses this after graph removal to prove the callback no
    /// longer owns this slot before retiring its permit cell.
    #[must_use]
    pub fn is_producer_gone(&self) -> bool {
        !self.rx.write_is_held()
    }
}

#[cfg(test)]
mod tests {
    use kithara_signal::TransportRevision;
    use kithara_test_utils::kithara;
    use kithara_warp::{BeatGridId, BeatGridRevision, BeatGridStamp};

    use super::*;
    use crate::{LoadGeneration, SyncOperationId, TopologyRevision, TopologyStamp};

    fn stamp() -> SyncExecutionStamp {
        let member = BeatGridId::allocate().expect("member id");
        let group = BeatGridId::allocate().expect("group id");
        SyncExecutionStamp::new(
            SyncOperationId::first(),
            BeatGridStamp::new(member, BeatGridRevision::first()),
            BeatGridStamp::new(group, BeatGridRevision::first()),
            TopologyStamp::new(group, TopologyRevision::first()),
            LoadGeneration::first(),
            TransportRevision::first(),
        )
    }

    fn rejected(stamp: SyncExecutionStamp) -> SyncReceipt {
        SyncReceipt::Rejected {
            stamp,
            reason: SyncExecutionReject::Late,
        }
    }

    #[kithara::test]
    fn a_kept_receipt_is_read_before_the_ring() {
        let (mut tx, mut inbox) = sync_receipts();
        let first = stamp();
        let second = stamp();
        tx.publish_rejected(first, SyncExecutionReject::Late)
            .expect("first rejection fits");
        let receipt = inbox.next_receipt().expect("first rejection");
        inbox.keep(receipt);
        tx.publish_rejected(second, SyncExecutionReject::Late)
            .expect("second rejection fits");

        assert_eq!(inbox.next_receipt(), Some(rejected(first)));
        assert_eq!(inbox.next_receipt(), Some(rejected(second)));
        assert_eq!(inbox.next_receipt(), None);
    }

    #[kithara::test]
    fn a_waiting_rejection_leaves_no_room_for_a_pair() {
        let (mut tx, mut inbox) = sync_receipts();
        let stamp = stamp();
        let applied = SyncApplied::builder()
            .stamp(stamp)
            .frontier(
                kithara_warp::PresentationFrontier::builder()
                    .source(0)
                    .output(kithara_signal::SessionFrame::new(0))
                    .build(),
            )
            .build();
        tx.publish_rejected(stamp, SyncExecutionReject::Late)
            .expect("rejection fits");
        assert!(tx.reserve_pair(applied).is_none());

        assert!(inbox.next_receipt().is_some());
        assert!(tx.reserve_pair(applied).is_some());
    }

    #[kithara::test]
    fn a_full_mailbox_hands_the_rejection_back() {
        let (mut tx, _inbox) = sync_receipts();
        let stamp = stamp();
        for _ in 0..consts::RECEIPT_PAIR {
            tx.publish_rejected(stamp, SyncExecutionReject::Late)
                .expect("rejection fits");
        }

        assert_eq!(
            tx.publish_rejected(stamp, SyncExecutionReject::Late),
            Err(rejected(stamp))
        );
    }

    #[kithara::test]
    fn the_inbox_sees_its_producer_gone_once_the_writer_drops() {
        let (tx, inbox) = sync_receipts();
        assert!(!inbox.is_producer_gone());
        drop(tx);
        assert!(inbox.is_producer_gone());
    }
}
