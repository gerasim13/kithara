# Dedicated CI host

The Mac mini is a CI-owned machine. Repository automation has one executable
owner: `xtask ci`. GitLab YAML and `just` recipes only select a typed Rust
command.

## Two configurations

CI reads two strict TOML files, and they have different owners:

| File | Owner | Tracked |
| --- | --- | --- |
| `.config/ci-pins.toml` | the repository | yes, reviewed with the code it pins |
| host profile (`host.toml`) | the machine | no, provisioned per host |

The pins hold everything a build depends on: toolchains, Xcode and Android
versions, `cargo install` versions, image tags and digests, download checksums.
The host profile holds what is true only for one machine: volume and cache
roots, disk thresholds, account names and UIDs, Homebrew and Xcode locations,
and the GitLab origin.

Every command takes `--config <host profile>`; `--pins` defaults to
`.config/ci-pins.toml` inside a checkout. Both accept the environment variables
`KITHARA_CI_HOST_CONFIG` and `KITHARA_CI_PINS`. Lanes read the host profile
only through `KITHARA_CI_HOST_CONFIG`, which every executor sets to
`/etc/kithara-ci/host.toml` (`C:/KitharaCI/host.toml` on Windows).

## Host installation

Write the machine profile for this host first — start from the field list in
`xtask/tests/fixtures/ci-host.toml` — and keep it outside the repository.
Then build the installer from a reviewed GitLab commit:

```text
export KITHARA_CI_HOST_CONFIG=/path/to/ci-host.toml
cargo build --locked --release -p xtask
sudo -E target/release/xtask ci host bootstrap
target/release/xtask ci host install-host-tools
sudo -E target/release/xtask ci host finish
```

`bootstrap` is idempotent. It validates the case-sensitive APFS volume, its
quota, user IDs, automatic login, SSH access, and the power policy before it
changes anything that already exists. `finish` validates Xcode and installs the
current Rust binary, the host profile, and the pins under
`/Volumes/KitharaCI/services`, and publishes the host profile to
`/etc/kithara-ci/host.toml` for the lanes.

Run the remaining commands in the logged-in `kithara-ci` GUI session, where the
installed copies are the source of truth:

```text
export KITHARA_CI_HOST_CONFIG=/Volumes/KitharaCI/services/host.toml
export KITHARA_CI_PINS=/Volumes/KitharaCI/services/pins.toml
/Volumes/KitharaCI/services/bin/kithara-ci ci host install-user-tools
/Volumes/KitharaCI/services/bin/kithara-ci ci host build-linux-image /path/to/kithara/docker/ci.Dockerfile
/Volumes/KitharaCI/services/bin/kithara-ci ci host smoke-linux
/Volumes/KitharaCI/services/bin/kithara-ci ci host smoke-android
```

The Linux image is built from the pins alone: `RUST_VERSION` and
`RUST_BASE_DIGEST` select the base image, and every tool version reaches the
Dockerfile as a build argument. No version is written twice.

## GitLab runners

Create four project runner authentication tokens in corporate GitLab:

| File under `~/.config/kithara-ci` | Tag | Executor |
| --- | --- | --- |
| `runner-macos.token` | `kithara-macos` | Cilicon disposable macOS VM |
| `runner-linux.token` | `kithara-linux` | pinned local Docker image |
| `runner-android.token` | `kithara-android` | macOS shell |
| `runner-release.token` | `kithara-release` | protected macOS shell |

Each token file must contain one `glrt-...` token and have mode `0600`. Also
create `~/.config/kithara-ci/cilicon-ssh.password` with the SSH password of the
pinned Cilicon image and mode `0600`; this value is written only to the local
`cilicon.yml`.

GitLab is reached over its public certificate chain, so runners, Cilicon
guests, and the bridge all validate `gitlab_url` against the platform trust
store. No private CA is installed or configured anywhere. A host that cannot
build that chain is a network fault to fix upstream, never a certificate to
add here and never a reason to relax TLS verification.

Then run:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host configure-runners
/Volumes/KitharaCI/services/bin/kithara-ci ci host activate
```

The generated runner configuration has `concurrent = 1`. The parent pipeline
also holds one GitLab `resource_group` until its child pipeline finishes, so
product pipelines cannot overlap even when a Windows runner is online.

## Windows

Windows uses an open-source UTM virtual machine stored below
`/Volumes/KitharaCI/vm/windows` with a 100 GB virtual-disk ceiling. Install the
official GitLab Runner inside the Windows 11 ARM guest with:

- one shell executor;
- tag `kithara-windows`;
- `concurrent = 1`;
- builds under `C:\KitharaCI\workspaces`;
- cache under `C:\KitharaCI\cache`;
- a copy of the host profile at `C:\KitharaCI\host.toml`.

The Windows installation media and license are intentionally not automated.
After the guest is registered, its job invokes the Rust CI command through
`cargo run --locked -p xtask -- ci run windows`; no PowerShell project script
is required. Windows runs only in the nightly chain, after Apple, Linux,
Android, and web checks.

## Repository bridge

Copy `ci/bridge/config.example.toml` to
`/Volumes/KitharaCI/services/bridge/config.toml`. The config, GitHub App key,
and GitLab token must belong to UID 504 (`kithara-sync`) and have mode `0600`.
Validate without network mutation:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge validate --config /Volumes/KitharaCI/services/bridge/config.toml
```

Activate the staged launch daemon only after validation:

```text
sudo -E /Volumes/KitharaCI/services/bin/kithara-ci ci host activate-bridge
```

GitLab `main` fast-forwards to GitHub only after the exact commit has a
successful GitLab push pipeline. A GitHub `main` update is imported only when
it belongs to a merged pull request, changes no CI control path, and passes the
private quarantine pipeline. Divergence is fail-closed and opens one
deduplicated GitLab incident.

## Storage policy

The CI volume has a 300 GB APFS quota. New work stops at 285 GB. Cleanup starts
at 240 GB and becomes aggressive at 270 GB. Workspaces, VM overlays, logs, and
whole inactive cache namespaces are pruned; individual Cargo, Gradle, and
sccache files are never deleted in place. Active jobs hold a 12-hour cache
lease, while sccache enforces its own 50 GB LRU limit.

Health and cleanup run through launchd. They can also be checked directly:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host health
/Volumes/KitharaCI/services/bin/kithara-ci ci host cleanup
```

## GitLab project settings

Protect `main`, release tags, the `release` environment, runner tokens, release
tokens, and bridge credentials. Keep release publication manual and restricted
to maintainers. Configure nightly and weekly schedules with
`KITHARA_SCHEDULE_KIND=nightly` and `KITHARA_SCHEDULE_KIND=weekly`.
