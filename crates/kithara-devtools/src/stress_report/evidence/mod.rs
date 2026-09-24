//! Correlates per-attempt stress failures with runtime evidence.

mod correlate;

use super::markdown_cell;

mod attempt;
mod divergence;
mod envelope;
mod line;
mod line_reader;
mod overlap;
mod pressure;

use correlate::{
    AttemptDossier, SignatureCluster, add_signature, clean_lines, duration_ms, normalize_signature,
    render_clusters, wait_signatures,
};
pub(super) use correlate::{
    append_correlated_evidence, backtrace_signature, parse_timestamp_ms, strip_ansi,
};
