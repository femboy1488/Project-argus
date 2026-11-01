Drop the bundled binaries here before `npm run tauri build`:

- adb / adb.exe (+ AdbWinApi.dll, AdbWinUsbApi.dll on Windows) from Android platform-tools
- llama-server / llama-server.exe built with -DGGML_RPC=ON (and -DGGML_CUDA=ON for NVIDIA)
- node.apk  — release build of ../../android (auto-installed on first connect)

IMPORTANT: llama-server here and rpc-server inside node.apk must come from the SAME llama.cpp commit.
The RPC protocol is versioned and mismatches are rejected.
