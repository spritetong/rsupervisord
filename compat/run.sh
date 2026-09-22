#!/usr/bin/env bash
#
# Run the rsupervisord compatibility suite.  Intended to be invoked inside WSL
# from the repo root:
#
#   bash compat/run.sh                       # default target: compiled rsupervisord
#   SUPERVISOR_TARGET=python bash compat/run.sh   # stock Python oracle
#   SUPERVISOR_STRICT=1 bash compat/run.sh   # unsupported => hard failures
#   bash compat/run.sh -m native             # only native supervisorctl tests
#   bash compat/run.sh -k xmlrpc             # subset
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PY="${SCRIPT_DIR}/.venv/bin/python"
TARGET="${SUPERVISOR_TARGET:-rsupervisord}"

if [ ! -x "${PY}" ]; then
  echo "==> venv missing, running bootstrap" >&2
  bash "${SCRIPT_DIR}/bootstrap.sh"
fi

if [ "${TARGET}" = "rsupervisord" ]; then
  if [ -n "${RSUPERVISORD_PROFILE:-}" ] && [ "${RSUPERVISORD_PROFILE}" != "debug" ]; then
    echo "==> cargo build --profile ${RSUPERVISORD_PROFILE}" >&2
    ( cd "${REPO_ROOT}" && cargo build --profile "${RSUPERVISORD_PROFILE}" )
  else
    echo "==> cargo build" >&2
    ( cd "${REPO_ROOT}" && cargo build )
  fi
fi

cd "${REPO_ROOT}"
exec "${PY}" -m pytest -q "${SCRIPT_DIR}/tests" "$@"
