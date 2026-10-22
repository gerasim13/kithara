# Android ELF runtime fixtures

Both AArch64 ELF images were built with NDK 29.0.14206865, API 24 clang,
from `main.c` and an empty `libtest-contract.rlib` archive. `context.so` also
links the repository `android/native-test/native/bridge.c` object with
`-shared`; `executable.so` retains `-pie` and the Android ELF interpreter.
The runtime classifier reads their program headers without executing them.

Rebuild with `aarch64-linux-android24-clang`:

```sh
llvm-ar rc libtest-contract.rlib
aarch64-linux-android24-clang -fPIC -c android/native-test/native/bridge.c -o bridge.o
aarch64-linux-android24-clang main.c libtest-contract.rlib -shared bridge.o -o context.so
aarch64-linux-android24-clang main.c libtest-contract.rlib -pie -o executable.so
```
