#!/bin/sh
# The host generates a just-in-time configuration per container. It registers
# one ephemeral runner, is valid for a single job, and expires with it, so no
# registration token or personal access token ever reaches this image.
set -eu

: "${ACTIONS_RUNNER_JITCONFIG:?the host must supply a just-in-time runner configuration}"

config="${ACTIONS_RUNNER_JITCONFIG}"
unset ACTIONS_RUNNER_JITCONFIG

exec ./run.sh --jitconfig "${config}"
