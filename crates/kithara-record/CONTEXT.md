# kithara-record context

## Ownership

`RecordingCore` owns one `EncoderSession`, one `ContainerSession`, and one
`RecordingSink` transaction. It owns no filesystem or AssetStore dependency.
`RecordingConfig` owns the selected `EncodeConfig`; WAV float32 remains the
portable encoder default.

## Transaction lifecycle

Construction validates the encode profile and, when known, preflights the
exact frame count against container limits. Any construction failure aborts
the supplied sink.

`push` accepts complete interleaved PCM frames, encodes available units, and
applies container writes at their declared offsets. Encode, container, frame
count, or sink failure is terminal: the transaction aborts and later pushes
return `Inactive`.

`finish` requires the expected frame count, flushes encoder tail units, writes
the container trailer and final header patches, then commits the exact final
length. Dropping an unfinished core aborts. A committed part is never modified
again and is independently readable.
