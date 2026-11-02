# Open the serial monitor on a connected timer node.
#
# PORT is the serial device (e.g. /dev/ttyUSB0). If omitted, espflash
# auto-detects the board (only reliable with a single board connected).
#
# Not runnable directly: nix/deploy.nix substitutes the @espflash@ store
# path and installs the result as `monitor` in the hardware deploy bundle
# (run via `nix run .#monitor` or the scripts/monitor.sh dispatcher).

ESPFLASH="@espflash@/bin/espflash"

port="${1:-}"

args=(monitor)
if [ -n "$port" ]; then
  args+=(--port "$port")
fi

exec "$ESPFLASH" "${args[@]}"
