#!/usr/bin/env bash
set -euo pipefail

real_linker=${MONOIZE_MUSL_LINKER:?MONOIZE_MUSL_LINKER must name the musl GCC linker}
rewritten_arguments=()

for argument in "$@"; do
  if [[ "$argument" == "-Wl,-Bdynamic" ]]; then
    rewritten_arguments+=("-Wl,-Bstatic")
  elif [[ "$argument" == "-lstdc++" ]]; then
    rewritten_arguments+=("-Wl,--start-group" "-lstdc++" "-lc" "-Wl,--end-group")
  else
    rewritten_arguments+=("$argument")
  fi
done

exec "$real_linker" "${rewritten_arguments[@]}"
