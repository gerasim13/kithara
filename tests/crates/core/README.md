# kithara-core-test-fixtures

This test-only package owns deterministic PCM vectors shared by low-level Kithara tests. Its fixtures cover ramps, silence, stereo pairs, and channel layouts without pulling the media fixture generator or encoded assets into core test builds.

Production crates may depend on it only from test targets. Add an input here when it represents a stable, reusable signal contract; keep scenario-specific data in the owning test package.
