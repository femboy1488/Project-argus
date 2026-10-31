#!/usr/bin/env bash
# How the shipped rpc-server builds were made (llama.cpp b12008, zig 0.13 as a static musl cross/native compiler).
# Produces three static aarch64 binaries in jniLibs; RpcService picks one at runtime from /proc/cpuinfo Features:
#   librpc_server.so          -mcpu=cortex_a53                          (armv8-a baseline, any arm64)
#   librpc_server_dotprod.so  -mcpu=generic+v8_2a+dotprod+fullfp16      (needs asimddp+asimdhp: A55/A75/A76+)
#   librpc_server_i8mm.so     -mcpu=generic+v8_6a+dotprod+fullfp16+i8mm (needs i8mm: A510/A710/X2+; no SVE emitted)
# Notes:
# - zig cc maps -march to a CPU name and rejects "armv8.2-a+dotprod", so features go in -mcpu (zig syntax) inside the
#   compiler wrapper and GGML_CPU_ARM_ARCH stays empty. ggml-cpu then probes __ARM_FEATURE_DOTPROD / _MATMUL_INT8 /
#   _FP16_VECTOR_ARITHMETIC (cfg log: "HAVE_DOTPROD - Success" etc.) and compiles the matching kernels.
# - Local patch required in ggml/src/ggml-rpc/transport.cpp: accept() -> accept4(fd, ..., SOCK_CLOEXEC);
#   Android's app seccomp filter kills plain accept() with SIGSYS.
set -euo pipefail
ZIG=${ZIG:?path to zig 0.13}; SRC=${SRC:?llama.cpp b12008 checkout}; WORK=${WORK:-$PWD}
OUT="$(cd "$(dirname "$0")/.." && pwd)/app/src/main/jniLibs/arm64-v8a"
declare -A CPU=( [base]=cortex_a53 [dotprod]=generic+v8_2a+dotprod+fullfp16 [i8mm]=generic+v8_6a+dotprod+fullfp16+i8mm )
for v in base dotprod i8mm; do
  printf '#!/bin/sh\nexec %s cc -target aarch64-linux-musl -mcpu=%s "$@"\n'  "$ZIG" "${CPU[$v]}" > "$WORK/zcc-$v"
  printf '#!/bin/sh\nexec %s c++ -target aarch64-linux-musl -mcpu=%s "$@"\n' "$ZIG" "${CPU[$v]}" > "$WORK/zcxx-$v"
  chmod +x "$WORK/zcc-$v" "$WORK/zcxx-$v"
  cmake -S "$SRC" -B "$WORK/b-$v" -G Ninja -DCMAKE_C_COMPILER="$WORK/zcc-$v" -DCMAKE_CXX_COMPILER="$WORK/zcxx-$v" \
    -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR=aarch64 -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
    -DCMAKE_EXE_LINKER_FLAGS=-static -DGGML_RPC=ON -DGGML_NATIVE=OFF -DGGML_OPENMP=OFF -DGGML_CCACHE=OFF \
    -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_UI=OFF
  ninja -C "$WORK/b-$v" ggml-rpc-server
  name=librpc_server.so; [ "$v" = base ] || name=librpc_server_$v.so
  cp "$WORK/b-$v/bin/ggml-rpc-server" "$OUT/$name"; strip "$OUT/$name"   # host strip works on aarch64 sandbox
done
ls -l "$OUT"
