# kithara-stream-tests

Integration tests for the public `kithara::stream` facade. The test sources
remain in [`tests/tests/kithara_stream`](../../tests/kithara_stream).

This package selects only the facade modules and in-memory source support needed
for stream tests. Keeping it separate prevents changes in unrelated domains
from rebuilding these test binaries.
