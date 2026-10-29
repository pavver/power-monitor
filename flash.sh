#!/usr/bin/env bash
# Builds the firmware and flashes it to the ESP32, then opens the serial monitor.
#
#   ./flash.sh                 test, build, flash, monitor (port auto-detected)
#   ./flash.sh -p /dev/ttyUSB0 use this serial port
#   ./flash.sh --reset         also erase saved settings (Wi-Fi, bot, texts)
#   ./flash.sh --no-monitor    do not open the monitor after flashing
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT_ARGS=()
ERASE="otadata" # boot the freshly flashed ota_0 even after an OTA update
MONITOR=(--monitor)

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p|--port) PORT_ARGS=(--port "$2"); shift 2 ;;
    --reset) ERASE="otadata,nvs"; shift ;;
    --no-monitor) MONITOR=(); shift ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "Невідомий параметр: $1 (див. --help)" >&2; exit 1 ;;
  esac
done

export PATH="$HOME/.cargo/bin:$PATH"
if [[ ! -f "$HOME/export-esp.sh" ]]; then
  echo "Немає ~/export-esp.sh — встановіть тулчейн: cargo install espup && espup install" >&2
  exit 1
fi
# shellcheck source=/dev/null
source "$HOME/export-esp.sh"
for tool in espflash ldproxy; do
  command -v "$tool" >/dev/null || { echo "Немає $tool — cargo install $tool --locked" >&2; exit 1; }
done

echo "==> Тести логіки (power-core)"
(cd "$ROOT/power-core" && cargo test --quiet)

echo "==> Збірка прошивки"
(cd "$ROOT/firmware" && cargo build --release)

echo "==> Прошивка (стерти: $ERASE)"
cd "$ROOT/firmware"
espflash flash "${PORT_ARGS[@]}" \
  --partition-table partitions.csv \
  --erase-parts "$ERASE" \
  "${MONITOR[@]}" \
  target/xtensa-esp32-espidf/release/power-monitor
