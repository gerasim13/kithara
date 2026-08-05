# A GitHub Actions runner that carries the CI toolchain in the image.
#
# Jobs therefore install nothing: no toolchain action, no package step, and no
# third-party action beyond the checkout itself. Point CI_IMAGE at the tag built
# from docker/ci.Dockerfile so a self-hosted job sees the same toolchain as
# every other lane.
ARG CI_IMAGE
FROM ${CI_IMAGE}

ARG ACTIONS_RUNNER_VERSION=2.336.0
ARG ACTIONS_RUNNER_AMD64_SHA256=04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d
ARG ACTIONS_RUNNER_ARM64_SHA256=58b758e420b87093fbd4bfddd368074960053e2f1388f01848c82624b90f27d1

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

COPY docker/ci-runner-entrypoint.sh /usr/local/bin/ci-runner
ENTRYPOINT ["/usr/local/bin/ci-runner"]
