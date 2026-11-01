#!/usr/bin/env bash
# Cross-compiles ggml rpc-server for Android arm64 and drops it into jniLibs as librpc_server.so.
# Usage: ANDROID_NDK=/path/to/ndk LLAMA_REF=<commit-or-tag> ./build-rpc-server.sh
# Use the SAME LLAMA_REF for the desktop llama-server, or the RPC handshake will fail.
set -euo pipefail
: "${ANDROID_NDK:?set ANDROID_NDK}"
LLAMA_REF="${LLAMA_REF:-b12008}"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-$HERE/../.build}"
OUT="$HERE/../app/src/main/jniLibs/arm64-v8a"

mkdir -p "$WORK" "$OUT"
if [ ! -d "$WORK/llama.cpp" ]; then
  git clone https://github.com/ggml-org/llama.cpp "$WORK/llama.cpp"
fi
git -C "$WORK/llama.cpp" fetch --tags origin && git -C "$WORK/llama.cpp" checkout "$LLAMA_REF"

cmake -S "$WORK/llama.cpp" -B "$WORK/build-android" \
  -DCMAKE_TOOLCHAIN_FILE="$ANDROID_NDK/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI=arm64-v8a \
  -DANDROID_PLATFORM=android-28 \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_RPC=ON \
  -DGGML_NATIVE=OFF \
  -DGGML_OPENMP=OFF \
  -DLLAMA_CURL=OFF \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF
cmake --build "$WORK/build-android" --target ggml-rpc-server -j"$(nproc)"

cp "$WORK/build-android/bin/ggml-rpc-server" "$OUT/librpc_server.so"
"$ANDROID_NDK"/toolchains/llvm/prebuilt/*/bin/llvm-strip "$OUT/librpc_server.so"
ls -lh "$OUT/librpc_server.so"
