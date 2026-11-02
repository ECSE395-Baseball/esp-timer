# Run a single timer node in QEMU, connected to a channel-sim hub.
#
# A channel-sim hub must already be listening on HUB_PORT (default: 7001 for
# start, 7002 for end). Interact via the serial console; exit with Ctrl-A X.
# --gui additionally opens an SDL window with the start node's emulated
# OLED (QEMU's virtual RGB panel); requires a graphical session.
#
# Not runnable directly: nix/qemu-sim.nix substitutes the @qemu@ and
# @firmware_qemu@ store paths and installs the result as `qemu-run` in the
# simulation bundle (run via `nix run .#qemu-run` or the
# scripts/qemu-run.sh dispatcher).

QEMU="@qemu@/bin/qemu-system-xtensa"
FIRMWARE_QEMU="@firmware_qemu@"

gui=0
args=()
for a in "$@"; do
  case "$a" in
  --gui)
    gui=1
    ;;
  *)
    args+=("$a")
    ;;
  esac
done
set -- "${args[@]:-}"

role="${1:-}"
case "$role" in
start)
  port="${2:-7001}"
  mac="52:54:00:12:34:01"
  ;;
end)
  port="${2:-7002}"
  mac="52:54:00:12:34:02"
  ;;
*)
  echo "usage: qemu-run [--gui] <start|end> [HUB_PORT]" >&2
  exit 2
  ;;
esac

# QEMU needs a writable flash file (NVS may format on first boot); the nix
# store is read-only, so boot from a copy.
flash=$(mktemp /tmp/timer-qemu-flash.XXXXXX.bin)
trap 'rm -f "$flash"' EXIT
cp -- "$FIRMWARE_QEMU/qemu-firmware-$role.bin" "$flash"
chmod u+w "$flash"

if [ "$gui" = 1 ]; then
  # -nographic implies serial-on-stdio; with a graphical display the serial
  # console must be requested explicitly.
  display_args=(-display sdl -serial stdio)
else
  display_args=(-nographic)
fi

exec "$QEMU" \
  -M esp32 "${display_args[@]}" \
  -drive file="$flash",if=mtd,format=raw \
  -nic socket,connect=127.0.0.1:"$port",model=open_eth,mac="$mac"
