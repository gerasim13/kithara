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

- Work the comment queue down: 426 comments carry prose outside a doc comment
  and 99 doc blocks run past a dozen lines. Only 76 sit above an item, where
  `--fix` moves them; the rest are inside bodies, where the answer is usually a
  named function rather than a sentence.
- `[lint_exclude]` still hides `kithara-devtools` from every style check, so
  446 of those comments are invisible to the ratchet. Narrow the exemption to
  the check files that carry lint patterns.
- Clear the 12 stale identifiers `doc_staleness` reports, then promote it to
  deny.
- Work through the 16 warn-level documents.

## Blocked

- Nothing.
