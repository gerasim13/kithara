# Dedicated CI host

The Mac mini is a CI-owned machine. Repository automation has one executable
owner: `xtask ci`. GitLab YAML and `just` recipes only select a typed Rust
command.

## Host installation

Build the installer from a reviewed GitLab commit:

```text
cargo build --locked --release -p xtask
sudo target/release/xtask ci host --config .config/ci-host.json bootstrap
target/release/xtask ci host --config .config/ci-host.json install-host-tools
sudo target/release/xtask ci host --config .config/ci-host.json finish
```

`bootstrap` is idempotent. It validates the case-sensitive APFS volume, its
quota, user IDs, automatic login, SSH access, and the power policy before it
changes anything that already exists. `finish` validates Xcode and installs the
current Rust binary plus its strict JSON configuration under
`/Volumes/KitharaCI/services`.

Run the remaining commands in the logged-in `kithara-ci` GUI session:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json install-user-tools
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json build-linux-image /path/to/kithara/docker/ci.Dockerfile
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json smoke-linux
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json smoke-android
```

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
`cilicon.yml`. Install the corporate CA as
`~/.config/kithara-ci/gitlab-ca.crt`, then run:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json configure-runners
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json activate
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
- cache under `C:\KitharaCI\cache`.

The Windows installation media and license are intentionally not automated.
After the guest is registered, its job invokes the Rust CI command through
`cargo run --locked -p xtask -- ci run windows`; no PowerShell project script
is required. Windows runs only in the nightly chain, after Apple, Linux,
Android, and web checks.

## Repository bridge

Copy `ci/bridge/config.example.json` to
`/Volumes/KitharaCI/services/bridge/config.json`. The config, GitHub App key,
and GitLab token must belong to UID 504 (`kithara-sync`) and have mode `0600`.
The GitLab CA may be read-only. Validate without network mutation:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge validate --config /Volumes/KitharaCI/services/bridge/config.json
```

Activate the staged launch daemon only after validation:

```text
sudo /Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json activate-bridge
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
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json health
/Volumes/KitharaCI/services/bin/kithara-ci ci host --config /Volumes/KitharaCI/services/host.json cleanup
```

## GitLab project settings

Protect `main`, release tags, the `release` environment, runner tokens, release
tokens, and bridge credentials. Keep release publication manual and restricted
to maintainers. Configure nightly and weekly schedules with
`KITHARA_SCHEDULE_KIND=nightly` and `KITHARA_SCHEDULE_KIND=weekly`.
