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

- Work the comment queue down by hand. `--fix` is exhausted - a second run on a
  clean tree changes nothing - so all 557 warnings are decisions: 426 comments
  carrying prose outside a doc comment, 99 doc blocks past a dozen lines, 24
  oversized inline comments, 8 dense functions. A body comment has no mechanical
  destination, and the answer there is usually a named function, not a sentence.
- `[lint_exclude]` still hides `kithara-devtools` from every style check, so
  446 of those comments are invisible to the ratchet. Narrow the exemption to
  the check files that carry lint patterns.
- Clear the 12 stale identifiers `doc_staleness` reports, then promote it to
  deny.
- Work through the 16 warn-level documents.

## Blocked

- Nothing.
