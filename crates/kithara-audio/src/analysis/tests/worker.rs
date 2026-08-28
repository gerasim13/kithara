use std::num::NonZeroU32;

use kithara_platform::CancelToken;
use kithara_resampler::NoResamplerBackend;
use kithara_test_utils::kithara;

use super::{
    super::{analyzer::AnalyzerBuilder, worker::AnalysisWorker},
    fixtures::{FakeReader, sine},
};

fn waveform_only() -> AnalyzerBuilder<NoResamplerBackend> {
    AnalyzerBuilder::<NoResamplerBackend>::default().with_waveform(16)
}

#[kithara::test(tokio)]
async fn delivers_result_on_its_own_thread() {
    let master = CancelToken::root();
    let worker = AnalysisWorker::new(&master, waveform_only());
    let (mut rx, _producer) = worker.analyze(
        Box::new(FakeReader::chunked(&sine(8192), 3)),
        worker.child_token(),
        "test-track".into(),
        super::fixtures::spec().sample_rate,
    );
    rx.changed().await.expect("worker sends a result");
    assert!(rx.borrow().as_ref().is_some_and(|a| a.waveform().is_some()));
}

#[kithara::test(tokio)]
async fn a_reader_on_another_axis_contributes_nothing() {
    let master = CancelToken::root();
    let worker = AnalysisWorker::new(&master, waveform_only());
    let axis = NonZeroU32::new(48_000).expect("test rate is non-zero");
    let (mut rx, _producer) = worker.analyze(
        Box::new(FakeReader::chunked(&sine(8192), 3)),
        worker.child_token(),
        "test-track".into(),
        axis,
    );

    assert!(
        rx.changed().await.is_err(),
        "a reader decoding onto another axis has nothing to contribute"
    );
    assert!(rx.borrow().is_none());
}

#[kithara::test(tokio)]
async fn preempted_job_sends_nothing_and_next_job_runs() {
    let master = CancelToken::root();
    let worker = AnalysisWorker::new(&master, waveform_only());

    let stale = worker.child_token();
    stale.cancel();
    let (mut stale_rx, _stale_producer) = worker.analyze(
        Box::new(FakeReader::chunked(&sine(8192), 3)),
        stale,
        "stale-track".into(),
        super::fixtures::spec().sample_rate,
    );

    let (mut live_rx, _live_producer) = worker.analyze(
        Box::new(FakeReader::chunked(&sine(8192), 3)),
        worker.child_token(),
        "live-track".into(),
        super::fixtures::spec().sample_rate,
    );
    live_rx.changed().await.expect("live job completes");
    assert!(live_rx.borrow().is_some());
    assert!(
        stale_rx.changed().await.is_err(),
        "preempted job must drop its sender without a result"
    );
    assert!(stale_rx.borrow().is_none());
}

#[kithara::test(tokio)]
async fn job_token_belongs_to_worker_scope() {
    let master = CancelToken::root();
    let worker = AnalysisWorker::new(&master, waveform_only());
    let job = worker.child_token();

    drop(worker);

    assert!(
        job.is_cancelled(),
        "dropping the worker must cancel tokens it creates for jobs"
    );
}
