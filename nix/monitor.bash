# Open the serial monitor on a connected timer node.
#
# PORT is the serial device (e.g. /dev/ttyUSB0). If omitted, espflash
# auto-detects the board (only reliable with a single board connected).

ESPFLASH="@espflash@/bin/espflash"

port="${1:-}"

args=(monitor)
if [ -n "$port" ]; then
  args+=(--port "$port")
fi

exec "$ESPFLASH" "${args[@]}"
