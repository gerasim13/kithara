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
        # The wasm target rebuilds std for shared memory, which only a nightly
        # honours, and the repository pins one. `wasm-pack` cannot be used
        # here: it drives Cargo with the default toolchain, which refuses the
        # flags below, so std keeps its stock features and the linker rejects
        # `--shared-memory` against it.
        browser="${1:-chrome}"
        # Which targets to run: everything, or only the crate-owned WebCodecs
        # one. The browser CI job wants the latter — `suite_heavy` costs minutes
        # it does not need to spend to say whether WebCodecs decodes.
        suite="${2:-all}"
        toolchain="${KITHARA_NIGHTLY_TOOLCHAIN:-nightly}"
        case "$suite" in
          all|webcodecs) ;;
          *)
            echo "unknown wasm test suite: $suite (use all|webcodecs)"
            exit 2
            ;;
        esac
        if [[ "$suite" == "webcodecs" && "$browser" != "chrome" ]]; then
          echo "the WebCodecs target runs on chrome only"
          exit 2
        fi
        case "$browser" in
          chrome)  export CHROMEDRIVER="${CHROMEDRIVER:-chromedriver}" ;;
          firefox) export GECKODRIVER="${GECKODRIVER:-geckodriver}" ;;
          safari)  export SAFARIDRIVER="${SAFARIDRIVER:-safaridriver}" ;;
          *)
            echo "unknown wasm test browser: $browser (use chrome|firefox|safari)"
            exit 2
            ;;
        esac
        if [[ "$suite" == "all" ]]; then
          WASM_BINDGEN_TEST_TIMEOUT=300 \
          WASM_BINDGEN_USE_BROWSER=1 \
          # The browser talks to a fixture server that runs on this machine, and
          # the runner looks for it beside itself. Nothing in a wasm build
          # produces a host binary, so the lane used to depend on some earlier
          # native job having left one behind, and failed with "failed to start
          # test_server ... No such file or directory" whenever it ran first.
          cargo build -p kithara-integration-tests --bin test_server
          # The integration browser tests live in `suite_heavy`. Building the
          # whole crate for wasm compiles every native suite too, and those reach
          # for threads, the filesystem, and a local test server that the browser
          # has no way to offer. The crate-owned WebCodecs target runs separately
          # below because it is not part of any integration suite.
          # `optimize_for_size` matches crates/kithara-ffi/.cargo/config.toml, so
          # the bundle lane and this one rebuild one identical std rather than
          # two that differ.
          # Keep the stress tests' own deadlines intact while running the WASM
          # graph with the workspace's release-grade test profile. `opt-level=2`
          # still missed the seek-pressure contract's 10-second deadline on the
          # self-hosted Chromium runner before reaching the final seek-to-zero
          # oracle.
          cargo "+$toolchain" test --profile test-release --target wasm32-unknown-unknown \
            -Z build-std=std,panic_abort,core,alloc \
            -Z build-std-features=optimize_for_size \
            -p kithara-integration-tests --test suite_heavy
        fi
        # `webcodecs_browser` belongs to kithara-decode, not to any integration
        # suite, so it is its own cargo invocation. WebCodecs itself is a
        # Chromium API; the other engines have nothing to answer here.
        if [[ "$browser" == "chrome" ]]; then
          CHROMEDRIVER="${CHROMEDRIVER:-chromedriver}" WASM_BINDGEN_TEST_TIMEOUT=300 \
          WASM_BINDGEN_USE_BROWSER=1 cargo "+$toolchain" test --target wasm32-unknown-unknown \
            -p kithara-decode --no-default-features \
            --features symphonia,webcodecs,client-reqwest,tls-rustls --test webcodecs_browser
        fi
        ;;
      build)
        just _xtask wasm build "$@"
        ;;
      size-check)
        just _xtask wasm size-check "$@"
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

# Apple tasks: xcframework, single, demo, xcode, ios, test,
# integration-regressions, doc, or release.
# `test` and `integration-regressions` resolve the iOS simulator themselves;
# set KITHARA_IOS_DESTINATION to an xcodebuild destination to pick one.
[positional-arguments]
apple MODE="xcframework" *ARGS:
    #!/usr/bin/env bash
    set -uo pipefail
    mode=$1
    shift

    ios_destination() {
      if [[ -n "${KITHARA_IOS_DESTINATION:-}" ]]; then
        printf '%s' "$KITHARA_IOS_DESTINATION"
        return 0
      fi
      xcrun simctl list --json | python3 -c 'import json, sys; data = json.load(sys.stdin); version = {r["identifier"]: r["version"] for r in data["runtimes"] if r.get("isAvailable")}; found = sorted((0 if d["state"] == "Booted" else 1, 0 if "iPhone" in d["deviceTypeIdentifier"] else 1, d["name"], version[runtime]) for runtime, devices in data["devices"].items() if ".SimRuntime.iOS-" in runtime and runtime in version for d in devices if d.get("isAvailable")); print("platform=iOS Simulator,name={},OS={}".format(found[0][2], found[0][3])) if found else sys.exit("no usable iOS simulator: searched every com.apple.CoreSimulator.SimRuntime.iOS-* runtime reported by `xcrun simctl list --json` for an available device on an available runtime, and found none. Create one under Xcode > Window > Devices and Simulators, or set KITHARA_IOS_DESTINATION to an xcodebuild -destination string.")'
    }

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
        # The debug framework carries one simulator slice, arm64 — the fat
        # slice is built for release. A generic simulator destination asks for
        # every simulator architecture, so the link failed with `symbol(s) not
        # found for architecture x86_64` against a framework that never
        # claimed to have them.
        KITHARA_LOCAL_DEV=1 xcodebuild -project apple/Examples/KitharaDemo/KitharaDemo.xcodeproj \
            -scheme "$scheme" -destination "$destination" ARCHS=arm64 build
        ;;
      test)
        destination=$(ios_destination) || exit 1
        test_server_url="${KITHARA_TEST_SERVER_URL:-}"
        if [[ -z "$test_server_url" ]]; then
          echo "KITHARA_TEST_SERVER_URL must name the running hermetic test server"
          exit 2
        fi
        # `--skip-build` is for a caller that already has the framework, which
        # in CI is a job that took it from the one that built it. The framework
        # has to be there: building it here anyway would hide the artifact
        # never arriving behind ten silent minutes.
        if [[ "${1:-}" == "--skip-build" ]]; then
          if [[ ! -d apple/KitharaFFIInternal.xcframework ]]; then
            echo "apple/KitharaFFIInternal.xcframework is missing; drop --skip-build or fetch it"
            exit 2
          fi
        else
          just platform apple xcframework --profile debug
        fi
        just platform _xcodegen-local
        KITHARA_LOCAL_DEV=1 \
        TEST_RUNNER_KITHARA_TEST_SERVER_URL="$test_server_url" \
          xcodebuild test \
            -project apple/Examples/KitharaDemo/KitharaDemo.xcodeproj \
            -scheme KitharaDemoUnitTests_iOS \
            -destination "$destination"
        ;;
      integration-regressions)
        # One xcodebuild invocation for the whole suite: the tests share a
        # process. Naming a test runs only that one.
        only_test="${1:-}"
        destination=$(ios_destination) || exit 1
        test_server_url="${KITHARA_TEST_SERVER_URL:-}"
        if [[ -z "$test_server_url" ]]; then
          echo "KITHARA_TEST_SERVER_URL must name the running hermetic test server"
          exit 2
        fi
        known=$(grep -l 'IntegrationRegressionsIOS' \
              apple/Examples/KitharaDemo/UnitTests/*.swift \
            | xargs grep -h -A1 '@Test(' \
            | grep -o 'func [A-Za-z0-9]*' | sed 's/^func //' | sort)
        if [[ -n "$only_test" ]]; then
          if ! grep -qxF "$only_test" <<< "$known"; then
            echo "unknown integration regression test: $only_test"
            echo "known tests:"
            sed 's/^/  /' <<< "$known"
            exit 2
          fi
        fi
        just platform apple xcframework --profile debug
        just platform _xcodegen-local
        KITHARA_LOCAL_DEV=1 xcodebuild build-for-testing \
            -project apple/Examples/KitharaDemo/KitharaDemo.xcodeproj \
            -scheme KitharaDemoUnitTests_iOS \
            -destination "$destination" || exit 1
        selection=(-only-testing:KitharaDemoUnitTests_iOS/IntegrationRegressionsIOS)
        log="$TMPDIR/kithara-integration-regressions.log"
        if [[ -n "$only_test" ]]; then
          selection=(
            "-only-testing:KitharaDemoUnitTests_iOS/IntegrationRegressionsIOS/${only_test}()"
          )
          log="$TMPDIR/kithara-integration-regressions-$only_test.log"
        fi
        rc=0
        KITHARA_LOCAL_DEV=1 TEST_RUNNER_KITHARA_TEST_SERVER_URL="$test_server_url" \
          xcodebuild test-without-building \
            -project apple/Examples/KitharaDemo/KitharaDemo.xcodeproj \
            -scheme KitharaDemoUnitTests_iOS \
            -destination "$destination" \
            "${selection[@]}" \
          > "$log" 2>&1 || rc=1
        grep -E '✔ Test "|✘ Test "|↳ ' "$log" || true
        echo "=== summary ==="
        echo "log: $log"
        if [[ $rc -eq 0 ]]; then
          echo "all regressions green"
        else
          echo "red — see the ✘ lines above"
          exit 1
        fi
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
        echo "unknown apple mode: $mode (use xcframework|single|demo|xcode|ios|test|integration-regressions|doc|release)"
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
