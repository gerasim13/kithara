# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. The grid is published at the tempo
  level the detector reports, tagged `grid_bpm_from_beats_v4`. Left: the
  reported deck scenario on a release build with the full model, and the size
  of the resume blob.

- `SpectralBeats`, a beat detector needing no model, beside the neural one. It
  searches the `Tempo` its caller hands it, and a build picks the model it
  embeds; the cache tag names both. Left: nothing.

- Harness and document revision. `AGENTS.md` routes instead of restating; the
  `style` namespace budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with
  `readme_shape`. All three queues are at zero.

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

- Work the comment queue down by hand: `--fix` is exhausted for comments, so
  all 668 are decisions (497 body comments, 105 long doc blocks, 50 oversized
  inline comments, 16 dense functions).
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.
- Wire `just lint style` to a gate: nothing runs it today. A warm run is 58 s,
  too much for every commit, nothing for a lane. The lane catalog owns that.

## Blocked

- Nothing.
