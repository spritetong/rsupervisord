#!/usr/bin/env bash
#
# Create the local Python virtualenv used by the supervisor compatibility
# harness and install the pinned toolchain into it.
#
# Runs on the WSL/Linux side (Python supervisord does not run natively on
# Windows). The venv lives next to this script as `.venv/` so the toolchain
# never touches the system PATH.
#
# Ubuntu prerequisite (once):  apt-get install -y python3-venv
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV="${SCRIPT_DIR}/.venv"
PY="${VENV}/bin/python"

echo "==> compat dir : ${SCRIPT_DIR}"

# 1. Remove any global (user/system) installs so PATH lookups resolve to
#    the local venv only.
python3 -m pip uninstall -y supervisor pytest >/dev/null 2>&1 || true

# 2. Create the local virtualenv (--copies avoids symlink quirks on 9p /mnt/*).
if [ ! -x "${PY}" ]; then
  echo "==> creating venv at ${VENV}"
  python3 -m venv --copies "${VENV}"
else
  echo "==> reusing venv at ${VENV}"
fi

# 3. Install pinned dependencies.
"${PY}" -m pip install --quiet --upgrade pip
"${PY}" -m pip install --quiet -r "${SCRIPT_DIR}/requirements.txt"

# 4. Report the locally installed toolchain.
echo "==> local toolchain:"
"${VENV}/bin/supervisord" --version
test -x "${VENV}/bin/supervisorctl" && echo "supervisorctl: present"
"${PY}" -m pytest --version

echo "==> done. use: ${VENV}/bin/<tool>"
