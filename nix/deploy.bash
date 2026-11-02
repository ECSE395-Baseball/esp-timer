# Flash a timer node with the hardware firmware, then open the serial
# monitor.
#
# PORT is the serial device (e.g. /dev/ttyUSB0). If omitted, espflash
# auto-detects the board (only reliable with a single board connected).
#
# Not runnable directly: nix/deploy.nix substitutes the @espflash@ and
# @firmware@ store paths and installs the result as `deploy` in the hardware
# deploy bundle (run via `nix run .#deploy` or the scripts/deploy.sh
# dispatcher).

ESPFLASH="@espflash@/bin/espflash"
FIRMWARE="@firmware@"

role="${1:-}"
port="${2:-}"

case "$role" in
start | end) ;;
*)
  echo "usage: deploy <start|end> [PORT]" >&2
  exit 2
  ;;
esac

args=(flash "$FIRMWARE/firmware-$role.elf" --monitor)
if [ -n "$port" ]; then
  args+=(--port "$port")
fi

echo ">> espflash ${args[*]}"
exec "$ESPFLASH" "${args[@]}"
