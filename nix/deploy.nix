# Hardware deploy bundle: espflash, the hardware (non-QEMU) firmware images
# (default and camera variant), and the flash/monitor scripts
#
#   nix run .#deploy -- start [/dev/ttyUSB0]           # flash a node + monitor
#   nix run .#monitor -- [/dev/ttyUSB0]                # serial monitor only
{
  symlinkJoin,
  writeShellApplication,
  espflash,
  firmware,
}:

let
  substitute =
    text:
    builtins.replaceStrings
      [
        "@espflash@"
        "@firmware@"
      ]
      [
        "${espflash}"
        "${firmware}"
      ]
      text;
in
symlinkJoin {
  name = "deploy-tools";
  paths = [
    (writeShellApplication {
      name = "deploy";
      text = substitute (builtins.readFile ./deploy.bash);
    })
    (writeShellApplication {
      name = "monitor";
      text = substitute (builtins.readFile ./monitor.bash);
    })
  ];
  meta.mainProgram = "deploy";
}
