#!/usr/bin/env bash
set -euo pipefail

live=1
if [[ "${1:-}" == "--offline" ]]; then
  live=0
  shift
fi

if [[ "$#" -lt 2 ]]; then
  echo "usage: verify-artifact.sh [--offline] <workspace> <name> [command ...]" >&2
  exit 2
fi

workspace=$1
name=$2
shift 2
fiasco_bin=${FIASCO_BIN:-fiasco}

"$fiasco_bin" --workspace "$workspace" mcp check "$name"
if [[ "$live" -eq 1 ]]; then
  "$fiasco_bin" --workspace "$workspace" mcp check "$name" --live
fi

for command in "$@"; do
  "$fiasco_bin" --workspace "$workspace" mcp compile "$command"
done
