#!/usr/bin/env bash
# Builds the firmware and flashes it to the ESP32, then opens the serial monitor.
#
#   ./flash.sh                 test, build, flash, monitor (port auto-detected)
#   ./flash.sh -p /dev/ttyUSB0 use this serial port
#   ./flash.sh --reset         also erase saved settings (Wi-Fi, bot, texts)
#   ./flash.sh --no-monitor    do not open the monitor after flashing
#   ./flash.sh --image         only build the images: power-monitor.bin for the web
#                              OTA update, power-monitor-full.bin for a first flash at 0x0
#   REMOTE_SERVER=https://…, DIVIDER_TOP_OHM=… DIVIDER_BOTTOM_OHM=…
#                              defaults built in for new devices (see README)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
PORT_ARGS=()
ERASE="otadata" # boot the freshly flashed ota_0 even after an OTA update
MONITOR=(--monitor)
IMAGE_ONLY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p|--port) PORT_ARGS=(--port "$2"); shift 2 ;;
    --reset) ERASE="otadata,nvs"; shift ;;
    --no-monitor) MONITOR=(); shift ;;
    --image) IMAGE_ONLY=1; shift ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
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

cd "$ROOT/firmware"
ELF=target/xtensa-esp32-espidf/release/power-monitor

if [[ $IMAGE_ONLY == 1 ]]; then
  espflash save-image --chip esp32 "$ELF" "$ROOT/power-monitor.bin"
  # Bootloader, partition table and app in one 4 MB file, settings erased:
  # what a new board needs, written at 0x0 by any flashing tool.
  espflash save-image --chip esp32 --merge --flash-size 4mb \
    --bootloader target/xtensa-esp32-espidf/release/bootloader.bin \
    --partition-table partitions.csv \
    "$ELF" "$ROOT/power-monitor-full.bin"
  echo "==> Готово: $ROOT/power-monitor.bin — завантажте його на панелі пристрою, розділ «Прошивка»."
  echo "    $ROOT/power-monitor-full.bin — повний образ для першої прошивки з адреси 0x0."
  exit 0
fi

echo "==> Прошивка через USB (стерти: $ERASE)"
# The bootloader built here has OTA rollback enabled; espflash's own does not.
espflash flash "${PORT_ARGS[@]}" \
  --bootloader target/xtensa-esp32-espidf/release/bootloader.bin \
  --partition-table partitions.csv \
  --erase-parts "$ERASE" \
  "${MONITOR[@]}" \
  "$ELF"
