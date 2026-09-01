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

- `crates/kithara-ui/CONTEXT.md` is the only denied document, at 155 KB against
  a 48 KB budget and four times the next largest crate contract.
- Clear the 18 stale identifiers `doc_staleness` reports, then promote it to
  deny.
- Work through the 16 warn-level documents.

## Blocked

- Nothing.
