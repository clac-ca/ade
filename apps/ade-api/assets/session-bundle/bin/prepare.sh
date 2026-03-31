#!/bin/sh
set -eu

mkdir -p "$(dirname "$ADE_PYTHON_HOME")"
if [ ! -x "$ADE_PYTHON_BIN" ]; then
  rm -rf "$ADE_PYTHON_HOME"
  mkdir -p "$ADE_PYTHON_HOME"
  tar -xzf "$ADE_PYTHON_TOOLCHAIN_PATH" -C "$ADE_PYTHON_HOME"
fi

exec "$ADE_PYTHON_BIN" -m pip install \
  --upgrade \
  --no-index \
  --find-links "$ADE_WHEELHOUSE" \
  "$ADE_CONFIG_WHEEL_PATH"
