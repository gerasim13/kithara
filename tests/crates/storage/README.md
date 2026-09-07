# kithara-storage-tests

Integration tests for the public `kithara::storage` facade. The test sources
remain in [`tests/tests/kithara_storage`](../../tests/kithara_storage).

This package selects only the facade modules and shared fixtures needed for
asset storage tests. Keeping it separate prevents changes in unrelated domains
from rebuilding these test binaries.
