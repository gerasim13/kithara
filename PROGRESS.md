# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Harness and document revision. `AGENTS.md` now routes instead of restating,
  and the `style` namespace budgets documents by size with `doc_size` and
  catches drift with `doc_staleness`.

## Next

- `comment_hygiene` cannot be trusted yet, for three separate reasons.
  `[lint_exclude]` hides whole crates from it, 292 `WHY:` prefixes satisfy the
  rule without removing anything, and its autofix now reaches nothing: all 138
  category violations are longer than the 30-character guard. Only the density
  check resists a marker, and it has no autofix by construction.
- Clear the 12 stale identifiers `doc_staleness` reports, then promote it to
  deny.
- Work through the 16 warn-level documents.

## Blocked

- Nothing.
