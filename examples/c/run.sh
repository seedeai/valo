#!/usr/bin/env bash
# Build the C API, compile the C example against it, and run it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo build -p valo-capi --release

# The staticlib needs the platform frameworks wgpu links against; the cdylib
# carries them itself, so link that and let the loader find it at runtime.
case "$(uname -s)" in
  Darwin) library_flags=(-L target/release -lvalo_capi -Wl,-rpath,"$root/target/release") ;;
  *)      library_flags=(-L target/release -lvalo_capi -Wl,-rpath,"$root/target/release") ;;
esac

cc -std=c11 -Wall -Wextra \
  -I crates/valo-capi/include \
  examples/c/hello.c \
  "${library_flags[@]}" \
  -o target/hello_c

exec ./target/hello_c
