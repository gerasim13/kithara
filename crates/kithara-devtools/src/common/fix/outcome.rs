/// Outcome of running a single check's `fix()` across the workspace.
///
/// `writes` is the number of files modified. `skipped` lists every scope
/// the fix refused to touch with a human-readable reason — typically a
/// floating comment that would be torn from its item by reordering, a
/// macro-bound item with unreliable spans, or `#[cfg]` adjacency that
/// could change semantics. A single fix run can both patch some files
/// and skip others; both pieces of information are surfaced.
#[derive(Debug, Default)]
pub struct FixOutcome {
    /// Human-readable description of each change the fix made (or, in a dry
    /// run, would make). Surfaced by the runner so `--fix` without `--apply`
    /// shows exactly what will be removed.
    pub changes: Vec<String>,
    pub skipped: Vec<String>,
    pub writes: usize,
}
