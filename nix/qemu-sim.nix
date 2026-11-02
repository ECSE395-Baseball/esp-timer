# QEMU simulation bundle: Espressif's QEMU fork, the complete QEMU firmware
# flash images, channel-sim, and the runner scripts with those store paths
# baked in — the whole simulation is one reproducible artifact:
#
#   nix run .#qemu-sim   # end-to-end two-node regression test
#   nix run .#qemu-run   # single interactive node (needs a running hub)
#
# The script sources live next to this file (qemu-sim.bash, qemu-run.bash)
# with @qemu@/@channel_sim@/@firmware_qemu@ placeholders, substituted here
# and shellchecked by writeShellApplication. scripts/qemu-{sim,run}.sh are
# thin `nix run` dispatchers for convenience.
{
  symlinkJoin,
  writeShellApplication,
  coreutils,
  gnugrep,
  gnused,
  gawk,
  qemu-esp32,
  firmware-qemu,
  channel-sim,
}:

let
  runtimeInputs = [
    coreutils
    gnugrep
    gnused
    gawk
  ];

  substitute =
    text:
    builtins.replaceStrings
      [
        "@qemu@"
        "@channel_sim@"
        "@firmware_qemu@"
      ]
      [
        "${qemu-esp32}"
        "${channel-sim}"
        "${firmware-qemu}"
      ]
      text;
in
symlinkJoin {
  name = "qemu-sim";
  paths = [
    (writeShellApplication {
      name = "qemu-sim";
      inherit runtimeInputs;
      text = substitute (builtins.readFile ./qemu-sim.bash);
    })
    (writeShellApplication {
      name = "qemu-run";
      inherit runtimeInputs;
      text = substitute (builtins.readFile ./qemu-run.bash);
    })
  ];
  meta.mainProgram = "qemu-sim";
}
