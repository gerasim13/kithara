# kithara-mpa

MPEG audio (MP1/MP2/MP3) demuxer for Symphonia, forked so a packet read is a
byte-exact transaction over a non-blocking source.

This crate is a fork of `symphonia-bundle-mp3` 0.6.0's `MpaReader` (MPL-2.0,
see `LICENSE`). It carries no decoder: `MpaDecoder` comes from the upstream
crate through Symphonia's codec registry, unchanged.

Only the demuxer is forked, and only to add rollback on a transient read.
Everything else tracks upstream verbatim so the delta stays reviewable.

See `CONTEXT.md` for the rollback contract, the fork's exact delta, and how to
rebase it onto a new upstream release.
