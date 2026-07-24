use kithara_assets::{ReadSide, WriteSide};
use kithara_net::{NetError, Retryability};
use kithara_test_utils::kithara;

use super::{FilePhase, ResourceEngine};

impl ResourceEngine {
    /// Apply the result of a streaming fetch to the resource.
    ///
    /// * commits the resource once the full byte space is covered
    /// * on a transient error with bytes already received, leaves the
    ///   resource active so the peer's next `poll_next` issues a
    ///   Range GET for the remaining gap
    /// * on a terminal error (non-retryable, or hard failure with no
    ///   bytes received), fails+evicts the resource and reports it to
    ///   the lifecycle sink
    ///
    /// Header capture (`Content-Length` / `Content-Type`) happens
    /// eagerly in `on_response`, not here, so a reader blocked on the
    /// first byte sees the seeded total the instant `write_at` fires.
    #[kithara::probe(resume_from, bytes_written)]
    pub(crate) fn finalize_fetch(
        &self,
        resume_from: u64,
        bytes_written: u64,
        total_bytes: Option<u64>,
        err: Option<&NetError>,
    ) {
        if let Some(e) = err {
            let terminal =
                e.retryability() == Retryability::Fatal || (resume_from == 0 && bytes_written == 0);
            if terminal {
                let msg = e.to_string();
                self.fail_and_evict(&msg);
                self.sink.error(&msg);
            }
            return;
        }

        if self.next_gap_start(total_bytes).is_some() {
            return;
        }

        let final_len = total_bytes.unwrap_or(resume_from + bytes_written);
        let Some(writer) = self.take_writer() else {
            // Writer already consumed (committed by a sibling/race) — the
            // resource is final; just advance the FSM.
            self.set_phase(FilePhase::Complete, self.reader.len());
            return;
        };
        match writer.commit(Some(final_len)) {
            Ok(reader) => self.set_phase(FilePhase::Complete, reader.len()),
            Err(e) => {
                let msg = e.to_string();
                self.fail_and_evict(&msg);
                self.sink.error(&msg);
            }
        }
    }
}
