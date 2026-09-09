# kithara-mpa

- Do not update the Symphonia stack independently of this vendored demuxer. Review the upstream delta together and preserve byte-exact transient frame rollback; timestamp recovery in kithara-decode must not also run for MPEG audio.

[Contract rationale](https://github.com/zvuk/kithara/wiki/kithara-mpa).
