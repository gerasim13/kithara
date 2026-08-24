use std::ops::Range;

use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::{SourcePhase, Stream};
use kithara_test_utils::kithara;

use super::rebuild::{TestConfig, TestControl, TestSource, TestStream, media_info};
use crate::pipeline::{
    decode::gate::source_phase_for_wait_context, stream::shared::SharedStream, track::WaitContext,
};

async fn shared_with_phase(
    phase: SourcePhase,
) -> (SharedStream<TestStream>, Arc<Mutex<Vec<Range<u64>>>>) {
    let source = TestSource::new(Arc::new(TestControl::new(media_info(0))));
    *source.phase_handle().lock() = phase;
    let waits = source.waits_handle();
    let stream = match Stream::<TestStream>::new(TestConfig { source }).await {
        Ok(stream) => stream,
        Err(error) => panic!("test stream construction failed: {error}"),
    };
    (SharedStream::new(stream), waits)
}

/// The playback wait polls the decoder's forward read-ahead window by phase.
/// A poll that files no demand leaves the parked reader invisible to the
/// source: dispatch budgets cover only ranges the source knows a reader
/// waits on (`Source::wait_range`), so the owed window never reaches the
/// polled segments and the fetch queue head sits one past the cap forever
/// (the phase_continuity livelock).
#[kithara::test(tokio)]
async fn a_parked_playback_wait_files_its_forward_window_as_demand() {
    let (shared, waits) = shared_with_phase(SourcePhase::Waiting).await;
    shared.set_position(1000);

    let phase = source_phase_for_wait_context(&shared, &WaitContext::Playback);

    assert_eq!(phase, SourcePhase::Waiting);
    // The forward window is position..position+read_ahead, clamped to the
    // source length (TestSource reports 4096).
    assert_eq!(
        waits.lock().clone(),
        vec![1000..4096],
        "a parked playback wait must file its polled window as reader demand"
    );
}

/// A ready poll is a snapshot, not a wait: it must not file demand, or the
/// hot produce path would re-file a look-ahead range on every tick.
#[kithara::test(tokio)]
async fn a_ready_playback_poll_files_no_demand() {
    let (shared, waits) = shared_with_phase(SourcePhase::Ready).await;
    shared.set_position(1000);

    let phase = source_phase_for_wait_context(&shared, &WaitContext::Playback);

    assert_eq!(phase, SourcePhase::Ready);
    assert!(
        waits.lock().is_empty(),
        "a ready poll must stay a pure snapshot"
    );
}
