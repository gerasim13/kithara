# A GitHub Actions runner that carries the CI toolchain in the image.
#
# Jobs therefore install nothing: no toolchain action, no package step, and no
# third-party action beyond the checkout itself. `CI_IMAGE` is the tag built
# from docker/ci.Dockerfile, so a self-hosted job sees the same toolchain as
# every other lane. Build it through `just ci image runner`, which passes every
# argument below from the reviewed pins.
ARG CI_IMAGE
FROM ${CI_IMAGE}

ARG ACTIONS_RUNNER_AMD64_SHA256
ARG ACTIONS_RUNNER_ARM64_SHA256
ARG ACTIONS_RUNNER_VERSION

# The runner is a .NET application and needs the ICU libraries at run time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    libicu72 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --shell /bin/bash runner \
 && install -d -o runner -g runner /runner

USER runner
WORKDIR /runner

RUN case "$(dpkg --print-architecture)" in \
      amd64) slice=x64; sum="${ACTIONS_RUNNER_AMD64_SHA256}" ;; \
      arm64) slice=arm64; sum="${ACTIONS_RUNNER_ARM64_SHA256}" ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL \
      -o /tmp/runner.tar.gz \
      "https://github.com/actions/runner/releases/download/v${ACTIONS_RUNNER_VERSION}/actions-runner-linux-${slice}-${ACTIONS_RUNNER_VERSION}.tar.gz" \
 && echo "${sum}  /tmp/runner.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/runner.tar.gz -C /runner \
 && rm /tmp/runner.tar.gz

# The just-in-time configuration is read once and dropped from the environment,
# so a job's own steps never inherit the credentials that registered them.
ENTRYPOINT ["/bin/sh", "-c", "config=${ACTIONS_RUNNER_JITCONFIG:?the host must supply a just-in-time runner configuration}; unset ACTIONS_RUNNER_JITCONFIG; exec ./run.sh --jitconfig \"$config\""]
