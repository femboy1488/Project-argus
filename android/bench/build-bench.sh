#!/usr/bin/env bash
# Builds argus-bench (static aarch64 musl, zig 0.13) in two flavours and copies them where they are used:
#   android/app/src/main/jniLibs/arm64-v8a/libargus_bench{,_dotprod}.so   (node app runs it for Wi-Fi beacons)
#   desktop/src-tauri/resources/argus-bench-a64{,-dotprod}                (desktop pushes it over adb for USB nodes)
set -euo pipefail
ZIG=${ZIG:?path to zig 0.13}
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$HERE/../.."
J="$ROOT/android/app/src/main/jniLibs/arm64-v8a"; R="$ROOT/desktop/src-tauri/resources"
$ZIG cc -target aarch64-linux-musl -mcpu=cortex_a53 -O2 -static -s -o "$HERE/argus-bench-a64" "$HERE/argus_bench.c" -lpthread
$ZIG cc -target aarch64-linux-musl -mcpu=generic+v8_2a+dotprod+fullfp16 -O2 -static -s -o "$HERE/argus-bench-a64-dotprod" "$HERE/argus_bench.c" -lpthread
cp "$HERE/argus-bench-a64" "$J/libargus_bench.so"; cp "$HERE/argus-bench-a64-dotprod" "$J/libargus_bench_dotprod.so"
cp "$HERE/argus-bench-a64" "$R/argus-bench-a64"; cp "$HERE/argus-bench-a64-dotprod" "$R/argus-bench-a64-dotprod"
ls -l "$HERE"/argus-bench-a64*
