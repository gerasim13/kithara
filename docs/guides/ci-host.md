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
| `runner-macos.token` | `kithara-macos` | macOS shell |
| `runner-linux.token` | `kithara-linux` | pinned local Docker image |
| `runner-android.token` | `kithara-android` | macOS shell |
| `runner-release.token` | `kithara-release` | protected macOS shell |

Each token file must contain one `glrt-...` token and have mode `0600`. The
macOS and Android shell executors run directly on the host. The Apple lane
therefore uses the host filesystem and cache roots across jobs instead of
booting an ephemeral machine for each build.

Apple packaging requires a case-folding checkout filesystem: Xcode creates a
`Headers` directory and `cargo-swift` addresses it as `headers`. When
`host_root` is case-sensitive, set `build_root` in the host profile to a
case-folding APFS location with enough build space. Bootstrap validates that
selected checkout root, and every runner uses it for `builds_dir`.

GitLab is reached over its public certificate chain, so runners and the bridge
validate `gitlab_url` against the platform trust store. No private CA is
installed or configured anywhere. A host that cannot build that chain is a
network fault to fix upstream, never a certificate to add here and never a
reason to relax TLS verification.

Then run:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host configure-runners
/Volumes/KitharaCI/services/bin/kithara-ci ci host activate
```

The generated runner configuration has `concurrent = 2`. Every runner exports
`CARGO_BUILD_JOBS=4`, so two Rust jobs use at most eight Cargo workers on the
ten-core host and leave capacity for the runner, sccache, linkers, and platform
tools. GitLab resource groups still serialize jobs that measure performance or
otherwise require exclusive ownership.

The GitLab Runner launch agent uses launchd's `Interactive` process type. Host
shell jobs inherit the runner's scheduling policy; marking that parent as
`Background` throttles both Cargo and the single-threaded source linters. Colima
remains a background service because Linux work receives its own container CPU
limit.

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
inspect open and merged pull requests, and `Commit statuses: write` to report
the verdict — check runs are a GitHub App API and the bridge does not use one.
Validate without network mutation:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge validate --config /Volumes/KitharaCI/services/bridge/config.toml
```

Activate the staged launch daemon only after validation:

```text
sudo -E /Volumes/KitharaCI/services/bin/kithara-ci ci host activate-bridge
```

The old GitLab pull mirror is disabled because it force-updated `main` and
could discard work already merged here. The bridge now moves `main` only by
fast-forward. When GitLab is ahead, it fast-forwards GitHub to the same commit;
when GitHub is ahead through a merged pull request, it refreshes both heads and
fast-forwards GitLab. Neither direction synthesizes a replacement commit or
force-pushes a diverged branch.

GitHub pull requests are verified before merge. While both `main` refs are
equal, the bridge reserves the exact pull-request head and base pair, publishes
one quarantine ref for that attempt, and starts its GitLab pipeline. The result
is written to the exact head commit under the status context
`kithara/gitlab-verification`. GitHub branch protection must require that
context on `main` and prevent direct pushes or bypasses; otherwise the verifier
is advisory and an unverified commit can still reach `main`.

A quarantine ref is addressed by the exact head and base pair it was judged
for. Once `main` moves, the next attempt reserves against the new base and
publishes a new ref, so nothing names the old branch again and nothing reads
its pipeline. Those branches are removed on the following tick; the pipelines,
jobs and artifacts they produced stay in GitLab's interface, which keys them by
commit rather than by branch.

A pull request that changes a CI control path is rejected before a pipeline is
created. Port that change through a GitLab merge request instead, so the code
that judges GitHub pull requests changes under GitLab review. Once a protected
GitHub pull request is merged, importing it is only the ordinary fast-forward
described above; there is no post-merge quarantine that can strand an already
accepted commit between the two forges.

A pipeline is judged on the child the dispatch stage triggers, never on its
parent: a parent reports `success` over a child that was cancelled. Divergence
is fail-closed and opens one deduplicated GitLab incident.

A rejection is recorded for the exact head and base pair and refused on sight.
Once its reason has been dealt with, clear that reservation so a new attempt
can run:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci bridge retry --config /Volumes/KitharaCI/services/bridge/config.toml \
  --github-sha <head-sha> --base-sha <base-sha>
```

## The verdict

The suite is not green, and gating on it being green holds every change behind
red it did not cause. The lanes the verdict judges therefore carry
`allow_failure: true` and one job decides: a run is held for failing something
the default branch is not.

Each lane leaves what it produced in `.ci-artifacts/junit/`, collected by the lane
dispatcher rather than by the lane itself — the build directory survives between
jobs, so the report a lane is expected to write is removed before it runs. A
lane that can name no test leaves a marker carrying its own name, because GitLab
hands a job no status for the jobs it needed.

The journal lives at `<shared cache root>/verdict/journal.json`, above the
trust- and platform-specific compiler caches. Each executor resolves that root
from its runner environment, so Linux containers and macOS shell jobs reach one
baseline rather than creating independent journals. `main` and the nightly
chain record; branch, merge-request and quarantine runs check. The window
unions the last five recorded runs: a test that fails a quarter of the time
would otherwise read as a regression whenever the run it was compared with
happened to be green.

A merge request is told which of the failures it is being let through with were
already failing at the commit it was branched from, when the journal still
remembers that commit.

The first check after this lands has no journal to read and says so; a `main`
run has to record before a regression can be told from what the branch already
carries.

## Storage policy

Pressure is read from what is left, not from what was spent. The thresholds
below are written as bytes used against the quota, and cleanup takes each one
as the free space it intends to keep — a volume at the reject threshold is one
with 15 GB to spare, which is exactly what a job's preflight requires. On an
APFS container the volume shares with others the two are different questions,
and reading them the old way left a 279 GB volume with 24 GB free reported as
`Normal` while jobs were already being refused.

The Linux guest keeps its root filesystem mounted `discard`, so that image stays
at about a gigabyte. Its data disk — the one carrying `/var/lib/docker` — is not,
so every layer Docker deletes stays allocated in a sparse file this volume pays
for. Every cleanup therefore trims it, whatever the pressure: the trim costs
seconds, and one on a machine that had drifted to 44 GB free returned 63 of them.
Recycling the guest outright reaches only what Docker still holds and the prune
window will not take, which is why refusal is the one thing that does it.

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

Health and cleanup run through launchd. They can also be checked directly:

```text
/Volumes/KitharaCI/services/bin/kithara-ci ci host health
/Volumes/KitharaCI/services/bin/kithara-ci ci host cleanup
```

Health reports every agent that has to hold a process for work to arrive —
`colima` and `gitlab-runner` — and fails when one of them does not. An agent
under `KeepAlive` that dies on startup stays loaded and keeps being restarted,
so health checks for the process rather than treating a loaded launchd service
as proof that it can take work. From the outside a missing runner looks like
nothing at all: its jobs simply sit `pending`, and the pipeline reads as hung.

## GitLab project settings

Protect `main`, release tags, the `release` environment, runner tokens, release
tokens, and bridge credentials. Keep release publication manual and restricted
to maintainers. Configure nightly and weekly schedules with
`KITHARA_SCHEDULE_KIND=nightly` and `KITHARA_SCHEDULE_KIND=weekly`.
