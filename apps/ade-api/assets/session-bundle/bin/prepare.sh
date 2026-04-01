#!/bin/sh
set -eu

app_root=/app/ade
python_home=/mnt/data/ade/python/current
python_bin="$python_home/bin/python3"

mkdir -p "$(dirname "$python_home")"
if [ ! -x "$python_bin" ]; then
  set -- "$app_root"/python/*.tar.gz
  if [ ! -f "$1" ]; then
    echo "Python toolchain bundle was not found under $app_root/python." >&2
    exit 1
  fi

  rm -rf "$python_home"
  mkdir -p "$python_home"
  tar -xzf "$1" -C "$python_home"
fi

exec "$python_bin" -m pip install \
  --upgrade \
  --no-index \
  --find-links "$app_root/wheelhouse/base" \
  "$app_root"/config/*.whl
