# kithara-drm — Context

Contracts and invariants for the kithara-drm crate; the README is the overview.

## Segment Decryption

HLS wraps `DecryptContext` as a `kithara-assets` `ResourceProcessor` / `ChunkSink` pair in `kithara-hls/src/decrypt_processor.rs`. Decryption runs once, on resource commit, inside `kithara-assets`' processing writer:

1. The committed resource is read back in 64 KB chunks (`processing::writer::CHUNK_SIZE`).
1. Each chunk goes through `aes128_cbc_process_chunk()`, from a pooled input buffer into a pooled output buffer.
1. The plaintext is written back into the same resource at a running output offset.
1. The final chunk (`is_last = true`) strips PKCS7 padding and returns the shorter plaintext length, which becomes the resource's committed length.

Invariants:

- Input length must be a multiple of the 16-byte AES block size; an unaligned chunk is a hard error.
- Chunks are not independent: an intermediate chunk decrypts with `NoPadding` and advances `ctx.iv` to its own last ciphertext block, so chunks must be fed in order by one `ChunkSink`. `ResourceProcessor::begin()` hands out a fresh sink (cloned `DecryptContext`) per commit so a retried commit restarts from the segment IV.
- No heap allocation on the decrypt path; the two 64 KB staging buffers come from the caller's registered `u8` pool in `PoolRegion`.
- `DecryptProcessor::identity()` is the `key || iv` bytes — that is what makes a cached processed resource identifiable.
- `DecryptContext` and `DecryptProcessor` redact key material from `Debug`.

## Key Derivation

IV derivation happens in `kithara-hls`'s `KeyStore`:

- If `#EXT-X-KEY` provides an explicit IV, it is used directly.
- Otherwise the IV is derived from the segment sequence number: `[0u8; 8] || sequence.to_be_bytes()`.
- Key and IV are both 16 bytes; a fetched key of any other length is rejected.
- Optional key unwrapping/processing for in-house DRM also happens in `KeyStore`, before a `DecryptContext` is built. Only the final plaintext key is cached (in memory and on disk); request-specific wire material never touches disk.

## Key Request Resolution

`kithara-drm` owns the policy-neutral request contract and its registry, not domain or URL policy:

- `KeyRequestResolver::prepare` optionally returns a final wire URL, policy headers, and the processor paired with that response.
- `KeyProcessorRegistry` consults resolvers in registration order and returns the first prepared request.
- `None` means the key stays on the plain AES-128 path.
- `KeyRequestFactory` produces fresh per-fetch headers and a processor derived from the same request material. Consumers must not reuse that pair across fetches.

`PreparedKeyRequest` redacts its URL, headers, and processor from `Debug` because URLs and headers may carry credentials. Concrete domain matching, query shaping, and static provider headers belong to the composition layer: `kithara-play`'s `DomainKeyPolicy` implements `KeyRequestResolver`, and `kithara-app` builds the registry.
