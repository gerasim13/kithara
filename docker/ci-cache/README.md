# CI compiler cache

Host-local S3 storage for independent sccache processes. The Compose file is
[../ci-cache.compose.yml](../ci-cache.compose.yml). The image digest is owned by
[../../.config/ci-pins.toml](../../.config/ci-pins.toml).

## Start

Copy the matching `linux.env.example` or `macos.env.example` to a file outside
the checkout. Review the data volumes, existing Docker network, client
endpoint, job UID, and per-scope quota. On macOS select the running Colima
context before these commands. Homebrew Compose also needs its plugin linked
for the CI account:

```sh
mkdir -p ~/.docker/cli-plugins
ln -s /opt/homebrew/lib/docker/cli-plugins/docker-compose ~/.docker/cli-plugins/docker-compose
ln -s /opt/homebrew/lib/docker/cli-plugins/docker-buildx ~/.docker/cli-plugins/docker-buildx
export DOCKER_CONTEXT=colima-kithara
docker network create kithara-ci-cache
```

The Linux example uses the already provisioned `kithara-ci` network.

```sh
just ci cache /absolute/path/cache.env up -d --build
just ci cache /absolute/path/cache.env logs initialize
```

The recipe reads the reviewed image pin and calls Docker Compose. Equivalent
direct use after exporting KITHARA_CACHE_IMAGE, KITHARA_RUST_VERSION and
KITHARA_RUST_DIGEST from ci-pins.toml:

```sh
docker compose --env-file /absolute/path/cache.env -f docker/ci-cache.compose.yml up -d --build
```

Compose creates persistent administrator credentials, starts MinIO, and initializes
one bucket and restricted writer per scope. Repeating `up` preserves credentials
and stored objects. Check that `initialize` exits successfully: detached startup
alone does not prove initialization.

Linux uses the example's absolute bind paths on the selected disk. macOS uses
persistent named Docker volumes inside Colima, avoiding unshared host paths.
Both survive container recreation. Do not use `down --volumes` unless intentionally
deleting the named stores. The image builds the existing Rust xtask for credential initialization and
verification; no checkout directories are mounted into containers.
The API is published only on host loopback; CI containers also reach it on the
selected Docker network. No console is published. Do not expose this HTTP endpoint
outside the host and its CI network.

## Connect jobs

Initialization writes `/clients/<scope>/cache.env` in its persistent clients
volume with mode 0600 and ownership `CACHE_CLIENT_UID`. Linux bind mounts expose
that file directly; on macOS export the selected scope to a private local file:

```sh
just ci cache /absolute/path/cache.env cp initialize:/clients/review/cache.env /private/path/review.env
chmod 600 /private/path/review.env
```

Pass only the appropriate scope to each executor:

- Docker runners: add `--env-file /absolute/path/clients/<scope>/cache.env`.
- Native macOS jobs: export variables from their scope's file before starting
  sccache. Use a job-specific daemon socket so existing daemons cannot retain
  another job's credentials.

Never mount the administrator directory or another scope's credentials into a
runner. Keep GitLab trusted, review, and quarantine scopes separate. Credentials
and generated environment files must not be committed.

Existing jobs are not reconfigured by Compose. Executor rollout and cache-hit
verification are a separate step, preserving active jobs.

## Verify

Run with Rust and sccache on PATH in the executor that will use the endpoint:

```sh
just ci cache-verify /absolute/path/clients/<scope>/cache.env
```

The probe starts two private daemons, initializes B before A writes, then checks
that B consumes A's output without restart or recompilation. It stops only its
own daemons and removes only its temporary files.

## Retention and scope

Each bucket has the configured hard quota and seven-day expiry. The examples
reserve at most 100 GiB across two Linux scopes and 3 GiB across three Mac scopes.
A quota is a storage limit, not LRU eviction: a full bucket rejects new writes
until objects expire. Size and retention require workload verification before
claiming sustained cache reuse.

This stack provides the compiler-cache backend. It does not distribute Cargo
target directories or cache final test linking, and does not establish a CI
speedup by itself.
