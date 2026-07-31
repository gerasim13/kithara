# kithara-drm — Context

Detailed contracts and invariants for the kithara-drm crate; the README is the overview.

## How It Works

HLS wraps `DecryptContext` as a `kithara-assets` `ResourceProcessor` / `ChunkSink`
pair in `kithara-hls/src/decrypt_processor.rs`. When an encrypted segment is
processed:

1. The segment data is read from disk in 64 KB chunks.
2. Each chunk is decrypted via `aes128_cbc_process_chunk()`.
3. The decrypted data is written back to the same location.
4. On the final chunk, PKCS7 padding is removed and the actual output length is returned.

Input must be aligned to the 16-byte AES block size. All operations are in-place (no buffer allocation).

## Key Derivation

IV derivation happens in `kithara-hls`'s `KeyStore`:

- If `#EXT-X-KEY` provides an explicit IV, it is used directly.
- Otherwise, IV is derived from the segment sequence number: `[0u8; 8] || sequence.to_be_bytes()`.
- Optional key unwrapping/processing for in-house DRM is also performed by `KeyStore` before building `DecryptContext`.

## Key Request Resolution

`kithara-drm` owns the policy-neutral request contract and its registry, not
domain or URL policy:

- `KeyRequestResolver::prepare` optionally returns a final wire URL, policy
  headers, and the processor paired with that response.
- `KeyProcessorRegistry` consults resolvers in registration order and returns
  the first prepared request.
- `None` means the key stays on the plain AES-128 path.
- `KeyRequestFactory` produces fresh per-fetch headers and a processor derived
  from the same request material. Consumers must not reuse that pair across
  fetches.

`PreparedKeyRequest` redacts its URL, headers, and processor from `Debug`
because URLs and headers may contain credentials. Concrete domain matching,
query shaping, and static provider headers belong to the composition layer
(`kithara-play`), which implements and registers `KeyRequestResolver`.
