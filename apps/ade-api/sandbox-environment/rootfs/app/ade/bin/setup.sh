#!/bin/sh
set -eu

app_root=/app/ade
config_root=/mnt/data/ade/config/current
python_home=/mnt/data/ade/python/current
python_bin="$python_home/bin/python3"
run_root=/app/ade/runs

if [ ! -x "$python_bin" ]; then
  echo "Pinned Python runtime was not found at $python_bin." >&2
  exit 1
fi

mkdir -p "$config_root" "$run_root"

set -- "$app_root"/wheelhouse/base/*.whl
if [ ! -f "$1" ]; then
  echo "Base wheelhouse was not found under $app_root/wheelhouse/base." >&2
  exit 1
fi

"$python_bin" -m pip install --upgrade --no-index --find-links "$app_root/wheelhouse/base" "$@"
