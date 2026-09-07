#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]

#[path = "file/early_stream_close.rs"]
mod early_stream_close;
#[path = "file/file_source.rs"]
mod file_source;
#[path = "file/html_error_cleanup.rs"]
mod html_error_cleanup;
#[path = "file/resume_stall_budget.rs"]
mod resume_stall_budget;
#[path = "file/seek_issues_range_request.rs"]
mod seek_issues_range_request;
#[path = "file/shared_download.rs"]
mod shared_download;
#[path = "file/waveform_shared_download.rs"]
mod waveform_shared_download;
