#!/usr/bin/env bash
set -euo pipefail

real_linker=${MONOIZE_MUSL_LINKER:?MONOIZE_MUSL_LINKER must name the musl GCC linker}
static_libgcc=${MONOIZE_MUSL_STATIC_LIBGCC:-false}
rewritten_arguments=()

if [[ "$static_libgcc" != "true" && "$static_libgcc" != "false" ]]; then
  echo "MONOIZE_MUSL_STATIC_LIBGCC must equal true or false" >&2
  exit 64
fi

for argument in "$@"; do
  if [[ "$argument" == "-Wl,-Bdynamic" ]]; then
    rewritten_arguments+=("-Wl,-Bstatic")
  elif [[ "$argument" == "-lstdc++" ]]; then
    rewritten_arguments+=("-Wl,--start-group" "-lstdc++" "-lc")
    if [[ "$static_libgcc" == "true" ]]; then
      rewritten_arguments+=("-lgcc")
    fi
    rewritten_arguments+=("-Wl,--end-group")
  else
    rewritten_arguments+=("$argument")
  fi
done

exec "$real_linker" "${rewritten_arguments[@]}"
