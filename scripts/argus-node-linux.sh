#!/usr/bin/env bash
# Argus Wi-Fi node for a Linux PC or Raspberry Pi.
#
# Downloads ggml rpc-server from llama.cpp b12008 (the same build as the Argus desktop app — the RPC
# protocol must match), runs `rpc-server -H 0.0.0.0 -p 50052` and broadcasts the Argus beacon every 2 s
# to UDP 50053:
#   {"argus":"node","v":1,"name":"<model>","port":50052,"ram_mb":N,"free_mb":N,"variant":"..."}
# The node then shows up in Argus → Cluster → "Найдено в сети" (button "Добавить").
#
# ⚠ rpc-server has NO password and NO encryption: closed home network only, never expose port 50052.
#
# Usage:  bash argus-node-linux.sh            run in the foreground (Ctrl+C stops everything)
#         bash argus-node-linux.sh --systemd  install + start a systemd --user service (autostart)
#         bash argus-node-linux.sh --uninstall-systemd
# Env:    ARGUS_PORT=50052 ARGUS_NODE_NAME="Pi 5" ARGUS_THREADS=4 ARGUS_CPU=1 (force CPU build on x64)
#         ARGUS_RPC_BIN=/path/to/rpc-server (skip download)  ARGUS_BEACON_TARGET=192.168.1.10 (unicast
#         beacon to the PC when broadcast is filtered)  ARGUS_NODE_DIR=~/.local/share/argus-node
set -euo pipefail

LLAMA_REF="${LLAMA_REF:-b12008}"
PORT="${ARGUS_PORT:-50052}"
BEACON_PORT=50053
DIR="${ARGUS_NODE_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/argus-node}"
THREADS="${ARGUS_THREADS:-$(nproc 2>/dev/null || echo 4)}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REL="https://github.com/ggml-org/llama.cpp/releases/download/${LLAMA_REF}"
mkdir -p "$DIR/cache"

log() { printf '[argus-node] %s\n' "$*" >&2; }
die() { log "ОШИБКА: $*"; exit 1; }

default_name() {
  local m=""
  [ -r /proc/device-tree/model ] && m="$(tr -d '\0' </proc/device-tree/model)"
  [ -z "$m" ] && [ -r /sys/class/dmi/id/product_name ] && m="$(cat /sys/class/dmi/id/product_name 2>/dev/null || true)"
  [ -z "$m" ] && m="$(hostname)"
  printf '%s' "$m"
}
NAME="${ARGUS_NODE_NAME:-$(default_name)}"

# ------------------------------------------------------------------ systemd helper
UNIT="$HOME/.config/systemd/user/argus-node.service"
if [ "${1:-}" = "--systemd" ]; then
  mkdir -p "$(dirname "$UNIT")"
  install -m 0755 "${BASH_SOURCE[0]}" "$DIR/argus-node-linux.sh"
  # keep the static aarch64 builds next to the installed copy (they are looked up relative to the script)
  for f in "$SCRIPT_DIR"/../android/app/src/main/jniLibs/arm64-v8a/librpc_server*.so; do [ -f "$f" ] && cp "$f" "$DIR/"; done
  cat >"$UNIT" <<UNIT_EOF
[Unit]
Description=Argus Wi-Fi node (llama.cpp ${LLAMA_REF} rpc-server + beacon)
After=network-online.target

[Service]
Environment=ARGUS_PORT=${PORT} ARGUS_NODE_NAME=${NAME// /\\x20} ARGUS_THREADS=${THREADS}
ExecStart=/usr/bin/env bash ${DIR}/argus-node-linux.sh
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
UNIT_EOF
  systemctl --user daemon-reload
  systemctl --user enable --now argus-node.service
  log "сервис установлен: systemctl --user status argus-node  (для автозапуска без входа: sudo loginctl enable-linger $USER)"
  exit 0
fi
if [ "${1:-}" = "--uninstall-systemd" ]; then
  systemctl --user disable --now argus-node.service 2>/dev/null || true
  rm -f "$UNIT"; systemctl --user daemon-reload || true
  log "сервис удалён"; exit 0
fi

# ------------------------------------------------------------------ pick / download rpc-server
fetch() { # url dest
  if command -v curl >/dev/null; then curl -fL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null; then wget -O "$2" "$1"
  else die "нужен curl или wget"; fi
}

from_release() { # asset-suffix variant
  local tgz="llama-${LLAMA_REF}-bin-$1.tar.gz" out="$DIR/${LLAMA_REF}-$2"
  if [ ! -x "$out/ggml-rpc-server" ]; then
    log "скачиваю ${tgz} …"
    rm -rf "$out" && mkdir -p "$out"
    fetch "$REL/$tgz" "$DIR/$tgz"
    tar -xzf "$DIR/$tgz" -C "$out" --strip-components=1
    rm -f "$DIR/$tgz"
    [ -x "$out/ggml-rpc-server" ] || die "в архиве нет ggml-rpc-server"
  fi
  BIN="$out/ggml-rpc-server"; VARIANT="$2"
}

BIN="${ARGUS_RPC_BIN:-}"; VARIANT="${ARGUS_VARIANT:-custom}"
if [ -z "$BIN" ]; then
  case "$(uname -m)" in
    x86_64|amd64)
      if [ "${ARGUS_CPU:-0}" != "1" ] && { command -v vulkaninfo >/dev/null && vulkaninfo --summary >/dev/null 2>&1 || [ -e /usr/lib/x86_64-linux-gnu/libvulkan.so.1 ]; }; then
        from_release ubuntu-vulkan-x64 x64-vulkan
      else
        from_release ubuntu-x64 x64-cpu
      fi ;;
    aarch64|arm64)
      # Our static musl builds (same as Argus Node on Android) run on any arm64 Linux, incl. Raspberry Pi OS.
      feats="$(grep -m1 -i '^features' /proc/cpuinfo 2>/dev/null || true)"
      pick=librpc_server.so; VARIANT=aarch64-baseline
      case " $feats " in *" i8mm "*) pick=librpc_server_i8mm.so; VARIANT=aarch64-i8mm;; *" asimddp "*) pick=librpc_server_dotprod.so; VARIANT=aarch64-dotprod;; esac
      for d in "$SCRIPT_DIR" "$SCRIPT_DIR/../android/app/src/main/jniLibs/arm64-v8a" "$DIR"; do
        if [ -f "$d/$pick" ]; then cp "$d/$pick" "$DIR/rpc-server-$VARIANT"; chmod +x "$DIR/rpc-server-$VARIANT"; BIN="$DIR/rpc-server-$VARIANT"; break; fi
        if [ -f "$d/librpc_server.so" ]; then cp "$d/librpc_server.so" "$DIR/rpc-server-aarch64-baseline"; chmod +x "$DIR/rpc-server-aarch64-baseline"; BIN="$DIR/rpc-server-aarch64-baseline"; VARIANT=aarch64-baseline; break; fi
      done
      if [ -z "$BIN" ]; then
        log "статическая сборка не найдена рядом со скриптом — беру официальный ubuntu-arm64 (нужен glibc ≥ 2.35)"
        from_release ubuntu-arm64 aarch64
      fi ;;
    *) die "архитектура $(uname -m) не поддерживается: соберите rpc-server ${LLAMA_REF} сами (-DGGML_RPC=ON) и задайте ARGUS_RPC_BIN" ;;
  esac
fi
[ -x "$BIN" ] || die "rpc-server не найден: $BIN"

# ------------------------------------------------------------------ beacon
mem_kb() { awk -v k="$1:" '$1==k {print $2}' /proc/meminfo; }
PIDFILE="$DIR/rpc.pid"

beacon_python() {
  ARGUS_NAME="$NAME" ARGUS_VARIANT="$VARIANT" ARGUS_PORT="$PORT" ARGUS_PIDFILE="$PIDFILE" \
  ARGUS_BEACON_PORT="$BEACON_PORT" ARGUS_TARGET="${ARGUS_BEACON_TARGET:-}" python3 - <<'PY'
import json, os, socket, time
name, variant = os.environ["ARGUS_NAME"], os.environ["ARGUS_VARIANT"]
port, bport, pidfile = int(os.environ["ARGUS_PORT"]), int(os.environ["ARGUS_BEACON_PORT"]), os.environ["ARGUS_PIDFILE"]
targets = ["255.255.255.255"] + [t for t in os.environ.get("ARGUS_TARGET", "").split(",") if t]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
def mem():
    m = {}
    with open("/proc/meminfo") as f:
        for l in f:
            k, v = l.split(":", 1); m[k] = int(v.split()[0])
    return m.get("MemTotal", 0) // 1024, m.get("MemAvailable", 0) // 1024
def alive():
    try:
        os.kill(int(open(pidfile).read().strip()), 0); return True
    except Exception:
        return False
while True:
    if alive():  # no beacon while rpc-server is down -> Argus marks the node OFFLINE after 10 s
        total, free = mem()
        msg = json.dumps({"argus": "node", "v": 1, "name": name, "port": port, "ram_mb": total,
                          "free_mb": free, "variant": variant}).encode()
        for t in targets:
            try: s.sendto(msg, (t, bport))
            except OSError: pass
    time.sleep(2)
PY
}

beacon_socat() {
  while true; do
    if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
      msg=$(printf '{"argus":"node","v":1,"name":"%s","port":%s,"ram_mb":%s,"free_mb":%s,"variant":"%s"}' \
        "${NAME//\"/}" "$PORT" "$(( $(mem_kb MemTotal) / 1024 ))" "$(( $(mem_kb MemAvailable) / 1024 ))" "$VARIANT")
      printf '%s' "$msg" | socat - "UDP-DATAGRAM:255.255.255.255:${BEACON_PORT},broadcast" 2>/dev/null || true
      for t in ${ARGUS_BEACON_TARGET//,/ }; do printf '%s' "$msg" | socat - "UDP-DATAGRAM:${t}:${BEACON_PORT}" 2>/dev/null || true; done
    fi
    sleep 2
  done
}

BEACON_PID=""
cleanup() {
  [ -n "$BEACON_PID" ] && kill "$BEACON_PID" 2>/dev/null || true
  [ -f "$PIDFILE" ] && kill "$(cat "$PIDFILE")" 2>/dev/null || true
  rm -f "$PIDFILE"
}
trap 'cleanup; exit 0' INT TERM
trap cleanup EXIT

if command -v python3 >/dev/null; then beacon_python & BEACON_PID=$!
elif command -v socat >/dev/null; then beacon_socat & BEACON_PID=$!
else log "нет python3 и socat — маяк выключен; добавьте ноду в Argus вручную: <IP>:${PORT}"; fi

IPS="$(hostname -I 2>/dev/null || true)"
log "нода «${NAME}» · ${VARIANT} · ${THREADS} потоков · порт ${PORT} · IP: ${IPS:-?}"
log "⚠ без пароля и шифрования — только закрытая домашняя сеть"

# ------------------------------------------------------------------ rpc-server, restarted if it dies
while true; do
  LLAMA_CACHE="$DIR/cache" LD_LIBRARY_PATH="$(dirname "$BIN")${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
    "$BIN" -H 0.0.0.0 -p "$PORT" -t "$THREADS" -c &
  echo $! >"$PIDFILE"
  wait "$(cat "$PIDFILE")" && code=0 || code=$?
  rm -f "$PIDFILE"
  log "rpc-server завершился (код $code) — перезапуск через 3 с"
  sleep 3
done
