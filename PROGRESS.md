# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Harness and document revision. `AGENTS.md` now routes instead of restating,
  and the `style` namespace enforces document budgets with `doc_size` and
  `doc_staleness`.

## Next

- Bring the deny-level documents under their limits: `crates/kithara-ui`,
  `crates/kithara-play`, `docs/guides/ci-host.md`, `tests/README.md`, and
  `apple/README.md`.
- Clear the stale identifiers `doc_staleness` reports at warn level, then
  promote the check to deny.
- Reduce the warn-level crate documents to their budgets.

## Blocked

- Nothing.
