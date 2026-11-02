# End-to-end emulation test: runs both timer nodes in QEMU with their link
# going through a channel simulator that adds delay/jitter/loss.
#
# The start node auto-fires 2s after clock sync; the end node auto-fires
# exactly 1.5s after the true start instant. The script checks two things:
#   * plumbing: the elapsed time computed by the start node matches 1.5s
#     within a tolerance (structurally exact unless the sync estimate drifts
#     mid-measurement, so this validates the event pipeline), and
#   * sync accuracy: the start node's clock-offset estimate is checked
#     against ground truth tracked by channel-sim, which knows the exact
#     delay it applied to every ping.
#
# Not runnable directly: nix/qemu-sim.nix substitutes the @qemu@,
# @channel_sim@ and @firmware_qemu@ store paths and installs the result as
# `qemu-sim` in the simulation bundle (run via `nix run .#qemu-sim` or the
# scripts/qemu-sim.sh dispatcher).

usage() {
  cat <<'EOF'
Usage: qemu-sim [--delay-ms D] [--jitter-ms J] [--loss-pct P]
                [--seed S] [--duration S] [--tolerance-ms T]
                [--sync-tolerance-ms T] [--gui]

--gui additionally opens an SDL window with the start node's emulated
OLED (QEMU's virtual RGB panel); requires a graphical session.
EOF
}

delay_ms=5
jitter_ms=1
loss_pct=0
seed=1
duration=30
tol_ms=50
sync_tol_ms=5
gui=0

while [ $# -gt 0 ]; do
  case "$1" in
  --delay-ms)
    delay_ms="$2"
    shift 2
    ;;
  --jitter-ms)
    jitter_ms="$2"
    shift 2
    ;;
  --loss-pct)
    loss_pct="$2"
    shift 2
    ;;
  --seed)
    seed="$2"
    shift 2
    ;;
  --duration)
    duration="$2"
    shift 2
    ;;
  --tolerance-ms)
    tol_ms="$2"
    shift 2
    ;;
  --sync-tolerance-ms)
    sync_tol_ms="$2"
    shift 2
    ;;
  --gui)
    gui=1
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    echo "unknown option: $1" >&2
    usage >&2
    exit 2
    ;;
  esac
done

QEMU="@qemu@/bin/qemu-system-xtensa"
HUB="@channel_sim@/bin/channel-sim"
FIRMWARE_QEMU="@firmware_qemu@"

logs=$(mktemp -d /tmp/timer-sim.XXXXXX)
pids=()
cleanup() {
  for pid in "${pids[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  echo ">> logs in $logs"
}
trap cleanup EXIT

echo ">> hub: delay=${delay_ms}ms jitter=${jitter_ms}ms loss=${loss_pct}% seed=$seed"
"$HUB" --delay-ms "$delay_ms" --jitter-ms "$jitter_ms" --loss-pct "$loss_pct" --seed "$seed" \
  7001 7002 >"$logs/hub.log" 2>&1 &
pids+=($!)
sleep 0.5

echo ">> starting nodes (duration ${duration}s)"
# QEMU needs writable flash files (NVS may format on first boot); the nix
# store is read-only, so boot from copies.
cp "$FIRMWARE_QEMU/qemu-firmware-start.bin" "$logs/flash-start.bin"
cp "$FIRMWARE_QEMU/qemu-firmware-end.bin" "$logs/flash-end.bin"
chmod u+w "$logs/flash-start.bin" "$logs/flash-end.bin"

if [ "$gui" = 1 ]; then
  # Show the start node's emulated OLED in an SDL window; serial goes to
  # the log file directly (with -nographic it is captured via redirection).
  "$QEMU" -M esp32 -display sdl -serial "file:$logs/start.log" \
    -drive file="$logs/flash-start.bin",if=mtd,format=raw \
    -nic socket,connect=127.0.0.1:7001,model=open_eth,mac=52:54:00:12:34:01 \
    2>"$logs/start-qemu.log" &
else
  "$QEMU" -nographic -M esp32 \
    -drive file="$logs/flash-start.bin",if=mtd,format=raw \
    -nic socket,connect=127.0.0.1:7001,model=open_eth,mac=52:54:00:12:34:01 \
    >"$logs/start.log" 2>&1 &
fi
pids+=($!)
"$QEMU" -nographic -M esp32 \
  -drive file="$logs/flash-end.bin",if=mtd,format=raw \
  -nic socket,connect=127.0.0.1:7002,model=open_eth,mac=52:54:00:12:34:02 \
  >"$logs/end.log" 2>&1 &
pids+=($!)

sleep "$duration"

# ---- analysis -------------------------------------------------------------
syncs=$(grep -c "clock sync OK" "$logs/start.log" || true)
elapsed_line=$(grep "ELAPSED" "$logs/start.log" | tail -1 || true)

echo ">> start node sync events: $syncs"
echo ">> start node: ${elapsed_line:-<no ELAPSED line>}"
grep "ELAPSED" "$logs/end.log" | tail -1 | sed 's/^/>> end node:   /' || true

if [ -z "$elapsed_line" ]; then
  echo "FAIL: no elapsed measurement was reported" >&2
  echo "--- start.log tail ---" >&2
  tail -20 "$logs/start.log" >&2
  echo "--- end.log tail ---" >&2
  tail -20 "$logs/end.log" >&2
  exit 1
fi

elapsed_s=$(sed -E 's/.*ELAPSED: ([0-9]+\.[0-9]+) s.*/\1/' <<<"$elapsed_line")
awk -v e="$elapsed_s" -v tol_ms="$tol_ms" '
  BEGIN {
    diff = e - 1.5;
    if (diff < 0) diff = -diff;
    if (diff * 1000 <= tol_ms) {
      printf "PASS: elapsed %.6f s, error %.3f ms within %d ms tolerance\n", e, diff * 1000, tol_ms;
      exit 0;
    } else {
      printf "FAIL: elapsed %.6f s, error %.3f ms exceeds %d ms tolerance\n", e, diff * 1000, tol_ms;
      exit 1;
    }
  }'

# ---- sync accuracy vs hub ground truth ------------------------------------
# channel-sim knows the exact delay it applied to every ping, so it can
# derive the true clock offset (offset = end_clock - start_clock) and
# compare it with the estimate the nodes actually used.
offset_line=$(grep "OFFSET_CHECK" "$logs/hub.log" | tail -1 || true)
truth_line=$(grep "OFFSET_TRUE" "$logs/hub.log" | tail -1 || true)
echo ">> hub: ${truth_line:-<no OFFSET_TRUE line>}"
echo ">> hub: ${offset_line:-<no OFFSET_CHECK line>}"

if [ -z "$offset_line" ]; then
  echo "FAIL: no ground-truth offset check was logged by channel-sim" >&2
  exit 1
fi

err_us=$(sed -E 's/.*error=(-?[0-9]+) us.*/\1/' <<<"$offset_line")
awk -v e="$err_us" -v tol_ms="$sync_tol_ms" '
  BEGIN {
    diff = e < 0 ? -e : e;
    if (diff <= tol_ms * 1000) {
      printf "PASS: offset estimate error %d us within %d ms sync tolerance\n", e, tol_ms;
      exit 0;
    } else {
      printf "FAIL: offset estimate error %d us exceeds %d ms sync tolerance\n", e, tol_ms;
      exit 1;
    }
  }'
