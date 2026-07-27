set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# WASM tasks: check, test, build, or size-check.
[positional-arguments]
wasm MODE="check" *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mode=$1
    shift
    case "$mode" in
      check)
        for c in kithara-storage kithara-net kithara-stream kithara-assets kithara-hls; do
          cargo check -p "$c" --target wasm32-unknown-unknown
        done
        cargo check -p kithara-decode --target wasm32-unknown-unknown --no-default-features --features symphonia,webcodecs,client-reqwest,tls-rustls
        cargo check -p kithara-ffi --target wasm32-unknown-unknown --features wasm --no-default-features
        ;;
      test)
        CHROMEDRIVER="${CHROMEDRIVER:-chromedriver}" \
        WASM_BINDGEN_TEST_TIMEOUT=300 \
        WASM_BINDGEN_USE_BROWSER=1 \
        cargo +nightly test --target wasm32-unknown-unknown -p kithara-integration-tests
        ;;
      build)
        just _xtask wasm build "$@"
        ;;
      size-check)
        toolchain="${WASM_SLIM_TOOLCHAIN:-nightly}"
        if command -v wasm-slim >/dev/null 2>&1; then
          slim_cmd="wasm-slim"
        elif command -v cargo-wasm-slim >/dev/null 2>&1; then
          slim_cmd="cargo wasm-slim"
        else
          echo "wasm-slim is not installed"; exit 2
        fi
        mkdir -p target
        ln -sfn ../../target crates/kithara-ffi/target
        cd crates/kithara-ffi
        RUSTUP_TOOLCHAIN="$toolchain" $slim_cmd build --check --no-emoji --json > ../../target/wasm-slim-result.json
        echo "wasm-slim report: target/wasm-slim-result.json"
        ;;
      *)
        echo "unknown wasm mode: $mode (use check|test|build|size-check)"
        exit 2
        ;;
    esac

# Android tasks: build, aar, or another xtask mode.
[positional-arguments]
android MODE="build" *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mode=$1
    shift
    case "$mode" in
      build)
        just _xtask android build "$@"
        ;;
      aar)
        just _xtask android aar
        ;;
      *)
        just _xtask android "$mode" "$@"
        ;;
    esac

# Apple tasks: xcframework, single, demo, xcode, ios, doc, or release.
[positional-arguments]
apple MODE="xcframework" *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mode=$1
    shift
    case "$mode" in
      xcframework)
        just _xtask apple build "$@"
        ;;
      single)
        just _xtask apple single "$@"
        ;;
      demo)
        just _xtask apple run "$@"
        ;;
      xcode)
        just platform apple xcframework --profile debug
        just platform _xcodegen-local
        open apple/Examples/KitharaDemo/KitharaDemo.xcodeproj
        ;;
      ios)
        scheme="${1:-KitharaDemo_iOS}"
        destination="${2:-generic/platform=iOS Simulator}"
        just platform apple xcframework --profile debug
        just platform _xcodegen-local
        KITHARA_LOCAL_DEV=1 xcodebuild -project apple/Examples/KitharaDemo/KitharaDemo.xcodeproj \
            -scheme "$scheme" -destination "$destination" build
        ;;
      doc)
        just _xtask apple docgen
        mkdir -p docs-build
        KITHARA_LOCAL_DEV=1 swift package --allow-writing-to-directory ./docs-build \
            generate-documentation \
            --target Kithara --target KitharaRx \
            --enable-experimental-combined-documentation \
            --output-path ./docs-build/Kithara.doccarchive
        ;;
      release)
        just _xtask apple release
        ;;
      *)
        echo "unknown apple mode: $mode (use xcframework|single|demo|xcode|ios|doc|release)"
        exit 2
        ;;
    esac

[private]
_xcodegen-local:
    cd apple/Examples/KitharaDemo && \
    sed -i.bak \
      -e 's|url: https://github.com/zvuk/kithara|path: ../../..|' \
      -e '/from:/d' \
      project.yml && \
    unset SDKROOT && \
    /usr/bin/xcrun --sdk macosx swift scripts/bake-secrets.swift && \
    KITHARA_LOCAL_DEV=1 xcodegen generate && \
    mv project.yml.bak project.yml
