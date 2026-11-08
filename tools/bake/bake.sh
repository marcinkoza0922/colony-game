#!/usr/bin/env sh
# Re-bakes all sprites into assets/sprites. Usage: tools/bake/bake.sh [--only NAME] [--debug]
set -e
cd "$(dirname "$0")/../.."
blender -b --factory-startup -P tools/bake/bake.py -- --out assets/sprites "$@" 2>&1 | grep -vE "OpenColorIO|^Fra:|Saved:|^\s*$" || true
