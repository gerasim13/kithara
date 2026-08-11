# Dedicated CI host

The Mac mini is a CI-owned machine. Repository automation has one executable
owner: `xtask ci`. GitLab YAML and `just` recipes only select a typed Rust
command.

## Two configurations

CI reads two strict TOML files, and they have different owners:

| File | Owner | Tracked |
| --- | --- | --- |
| `.config/ci-pins.toml` | the repository | yes, reviewed with the code it pins |
| host profile (`mac-host.toml`, `linux-host.toml`) | the machine | no, provisioned per host |

The pins hold everything a build depends on: toolchains, Xcode and Android
versions, `cargo install` versions, image tags and digests, download checksums.
The host profile holds what is true only for one machine: volume and cache
roots, disk thresholds, account names and UIDs, Homebrew and Xcode locations,
and the GitLab origin.

Every command takes `--config <host profile>`; `--pins` defaults to
`.config/ci-pins.toml` inside a checkout. Both accept the environment variables
`KITHARA_CI_HOST_CONFIG` and `KITHARA_CI_PINS`. Lanes read the host profile
only through `KITHARA_CI_HOST_CONFIG`, which every executor sets to
`/etc/kithara-ci/mac-host.toml` (`C:/KitharaCI/mac-host.toml` on Windows).

## Host installation

Write the machine profile for this host first — start from the field list in
`xtask/tests/fixtures/ci-mac-host.toml` (`ci-linux-host.toml` for a Linux
host) — and keep it outside the repository.

A Linux host declares the repositories it serves and the token that speaks for
each, then every runner names one of them:

```toml
[[repositories]]
name = "octocat/kithara"
token_file = "/etc/kithara-ci/tokens/octocat.token"

[[repositories]]
name = "hubot/kithara"
token_file = "/etc/kithara-ci/tokens/hubot.token"

[[runners]]
name = "kithara-ci-hubot"
repository = "hubot/kithara"
cpus = 12
memory = "48g"
labels = ["self-hosted", "linux", "x64", "kithara"]
```

No entry is the default and none is subordinate: the repositories are peers,
named after their owners, and a runner belongs to whichever one it names. This
is not a convenience — a GitHub runner registration reaches exactly one
repository, so serving a second one means separate runner processes holding a
separate credential.

One token per file, named after the owner it authorises. A shared file would
widen a leak to every repository on the machine and make rotating one token an
edit to the file the others depend on, and it would put a parsed format between
the profile and a secret. Create one with
`install -m 600 -o root -g root /dev/stdin /etc/kithara-ci/tokens/<owner>.token`,
which sets the mode as the file is created and keeps the token out of shell
history; `sudo` overrides an inherited `umask`, so `tee` leaves the file
world-readable unless it is chmodded afterwards.

A runner naming a repository the machine holds no credential for is refused
while the profile is read. Left to run it would register against whichever
token the machine happened to hold, come up, take work, and report to the wrong
repository.
Then build the installer from a reviewed GitLab commit:

```text
export KITHARA_CI_HOST_CONFIG=/path/to/ci-mac-host.toml
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
`/etc/kithara-ci/mac-host.toml` for the lanes.

Run the remaining commands in the logged-in `kithara-ci` GUI session, where the
installed copies are the source of truth:

```text
export KITHARA_CI_HOST_CONFIG=/Volumes/KitharaCI/services/mac-host.toml
export KITHARA_CI_PINS=/Volumes/KitharaCI/services/pins.toml
/Volumes/KitharaCI/services/bin/kithara-ci ci host install-user-tools
/Volumes/KitharaCI/services/bin/kithara-ci ci host build-linux-image /path/to/kithara/docker/ci.Dockerfile
/Volumes/KitharaCI/services/bin/kithara-ci ci host smoke-linux
/Volumes/KitharaCI/services/bin/kithara-ci ci host smoke-android
```

The Linux image is built from the pins alone: `RUST_VERSION` and
`RUST_BASE_DIGEST` select the base image, and every tool version reaches the
Dockerfile as a build argument. No version is written twice.

A Linux image tag change is not deployed until the Mac mini has built it and
regenerated its runner configuration. From a checkout of the commit carrying
the new pin, logged in as `kithara-ci` on that host, run:

```text
cargo build --locked --release -p xtask
export KITHARA_CI_HOST_CONFIG=/Volumes/KitharaCI/services/mac-host.toml
export KITHARA_CI_PINS=$PWD/.config/ci-pins.toml
target/release/xtask ci host build-linux-image $PWD/docker/ci.Dockerfile
target/release/xtask ci host configure-runners
target/release/xtask ci host activate
```

GitLab leaves the job image to that generated runner configuration. The runner
never pulls this local-only tag and declares the tag it provisioned to each
job; `xtask ci run` compares that declaration with the checked-out pin before
running a Linux, web, or dependency lane. A drifted host therefore starts its
existing local image only long enough to name the missing tag and print the
commands above.

## GitLab runners

Create four project runner authentication tokens in corporate GitLab:

| File under `~/.config/kithara-ci` | Tag | Executor |
| --- | --- | --- |
| `runner-macos.token` | `kithara-macos` | throwaway `tart` macOS VM |
| `runner-linux.token` | `kithara-linux` | pinned local Docker image |
| `runner-android.token` | `kithara-android` | macOS shell |
| `runner-release.token` | `kithara-release` | protected macOS shell |

Each token file must contain one `glrt-...` token and have mode `0600`. Also
create `~/.config/kithara-ci/macos-guest.password` with the SSH password of the
macOS guest account and mode `0600`.

The macOS lane runs each job in a throwaway VM. `xtask ci host run-macos-runner`
clones the base image named by `macos_vm_bundle`, boots the clone headless
with Xcode and the Rust toolchain mounted from the host, lets GitLab hand it a
single build through `gitlab-runner run-single --max-builds 1`, and destroys the
clone afterwards. Build that base image once, from the Apple restore image
matching `macos_guest_build`:

```text
tart create --from-ipsw <UniversalMac_<version>_<build>_Restore.ipsw> kithara-macos-base
tart run kithara-macos-base
```

Complete Setup Assistant in the guest, create the `macos_guest_user` account
with the password above, enable Remote Login, authorise the CI account's SSH
key, grant that account passwordless `sudo`, and accept the Xcode licence once
with the host Xcode mounted. The image carries no Xcode of its own, so it stays
small and always matches `expected_xcode_version` on the host.

GitLab is reached over its public certificate chain, so runners, macOS
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
- a copy of the host profile at `C:\KitharaCI\mac-host.toml`.

The Windows installation media and license are intentionally not automated.
After the guest is registered, its job invokes the Rust CI command through
`cargo run --locked -p xtask -- ci run windows`; no PowerShell project script
is required. Windows runs only in the nightly chain, after Apple, Linux,
Android, and web checks.

## Repository bridge

Copy `ci/bridge/config.example.toml` to
`/Volumes/KitharaCI/services/bridge/config.toml`. The config and the two
tokens must belong to UID 504 (`kithara-sync`) and have mode `0600`. The GitHub
token needs `Contents: write` to publish `main`, `Pull requests: read` to
recognise a merged pull request, and `Commit statuses: write` to report the
verdict — check runs are a GitHub App API and the bridge does not use one.
Validate without network mutation:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge validate --config /Volumes/KitharaCI/services/bridge/config.toml
```

Activate the staged launch daemon only after validation:

```text
sudo -E /Volumes/KitharaCI/services/bin/kithara-ci ci host activate-bridge
```

GitLab `main` fast-forwards to GitHub only after the exact commit has a
successful GitLab push pipeline, judged on the child pipeline the dispatch
stage triggers rather than on its parent. A GitHub `main` update is imported
only when it belongs to a merged pull request, changes no CI control path, and
passes the private quarantine pipeline. Manifests are not control paths:
`deps:deny` runs on quarantine instead. Divergence is fail-closed and opens one
deduplicated GitLab incident.

A rejection is recorded and refused on sight. Once its reason has been dealt
with, clear the head so the import runs again:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge retry --config /Volumes/KitharaCI/services/bridge/config.toml --github-sha <sha>
```

## The verdict

The suite is not green, and gating on it being green holds every change behind
red it did not cause. The lanes the verdict judges therefore carry
`allow_failure: true` and one job decides: a run is held for failing something
the default branch is not.

Each lane leaves what it produced in `target/junit/`, collected by the lane
dispatcher rather than by the lane itself — the build directory survives between
jobs, so the report a lane is expected to write is removed before it runs. A
lane that can name no test leaves a marker carrying its own name, because GitLab
hands a job no status for the jobs it needed.

The journal lives on the Linux executor, whose cache directory is mounted from
the host, at `<cache root>/verdict/journal.json`. A macOS job runs in a
throwaway guest, where nothing written survives it. `main` and the nightly chain
record; branch, merge-request and quarantine runs check. The window unions the
last five recorded runs: a test that fails a quarter of the time would otherwise
read as a regression whenever the run it was compared with happened to be green.

A merge request is told which of the failures it is being let through with were
already failing at the commit it was branched from, when the journal still
remembers that commit.

The first check after this lands has no journal to read and says so; a `main`
run has to record before a regression can be told from what the branch already
carries.

## Storage policy

The CI volume has a 300 GB APFS quota. New work stops at 285 GB. Cleanup starts
at 240 GB and becomes aggressive at 270 GB. Workspaces, VM overlays, logs, and
whole inactive cache namespaces are pruned; individual Cargo, Gradle, and
sccache files are never deleted in place. Active jobs hold a 12-hour cache
lease, while sccache enforces its own 50 GB LRU limit.

The quota cannot be raised in place: this macOS has no `diskutil apfs` verb for
it, and `-quota` is only accepted when a volume is created. Cleanup is
therefore the whole answer, and it is written against where the space goes
rather than against a list of names:

- A cache namespace nothing writes to any more is pruned once it has been
  quiet for a week, and immediately when jobs are already being refused. The
  named steps alone left six gigabytes of `cargo-reapi` stores behind after
  that tool came off the CI path: a namespace that stops being written to goes
  invisible rather than stale.
- The macOS guest grows on this volume for as long as it lives and returns the
  space only when it is thrown away — one recycle gave back 38 GiB, against
  three and a half for every other step together. A guest serves six jobs
  (`JobVm::MAX_BUILDS`) before it is replaced, and cleanup restarts the agent
  outright once the volume is above the refusal threshold. That costs the job
  in flight, which is why nothing below that threshold does it.

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
