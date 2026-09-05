# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Harness and document revision. `AGENTS.md` routes instead of restating, and
  the `style` namespace now budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with `readme_shape`:
  a header that stays inside the package, badges keyed to `publish` and to the
  manifest's license, a `# <package name>` title, then `Usage` / `Key Types` /
  `Features` / `Integration` and nothing else. All three queues are at zero, and
  the rewrites turned up claims the sources contradict - a wrong feature list, a
  file that no longer exists, an inverted description of a known leak, an MPL-2.0
  crate wearing the MIT badge, two crates naming a dead owner, and a logo no
  published crate page could load.

- Full-playthrough queue census. A queue is played from the first frame of the
  first track to the last frame of the last, and every output frame is
  attributed to the track that produced it: `PlayerTrack::render` carries a USDT
  probe naming the track, the block-relative span it was asked for, and the
  track's own media clock. Both halves of a premature switch are pinned - a
  track must serve its whole length, and two tracks may share frames only inside
  the crossfade the queue announced - and the rendered audio says the same twice
  more, by ramp provenance and by Cochlea. The census runs over every reader a
  track arrives through: HLS segments, a local FLAC file, a whole FLAC body over
  HTTP, and a whole MPEG body between two HLS tracks, each at cf=0 and cf>0.
  `real_playlist` gained the network counterpart - the real `silvercomet` HLS
  stream ahead of a real MPEG body, measuring where the outgoing track's clock
  stood when the queue left it.

  Two negative results, each confirmed twice: an HLS playlist duration is exact
  float arithmetic over `#EXTINF` and cannot be under-stated, and an MPEG
  duration is header-derived and can only be over-stated. The wrong-duration
  family is closed. A guard against an end the declared length does not account
  for was built and reverted - ten tests state the opposite contract outright,
  because a declared duration is routinely longer than the audio that decodes.

  The seam test found why nothing had caught the defect: `suite_network` has not
  compiled since `#260`, so every real-CDN test has been dark for a fortnight.
  The lane builds again. The reported defect is still open and its mechanism is
  not known.

## Next

- Work the comment queue down by hand. `--fix` is exhausted for comments - a
  second run on a clean tree changes nothing - so all 668 are decisions: 497
  comments carrying prose outside a doc comment, 105 doc blocks past a dozen
  lines, 50 oversized inline comments, 16 dense functions. A body comment has no
  mechanical destination.
- 439 ordering findings are still mechanical: `struct_field_order` 160,
  `trait_item_order` 188, `struct_init_order` 91. One `just lint style --fix`
  clears them, but it rewrites declarations across every crate, so it wants its
  own change.
- Wire `just lint style` to a gate. Nothing runs it today - not the commit hook,
  not a CI lane - which is why the ratchet drifted unseen. A warm run is 58 s:
  too much for every commit, nothing for a lane. The lane catalog owns that
  change, so it does not belong in this one.

## Blocked

- Nothing.
