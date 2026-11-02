# Hardware deploy bundle: espflash, the hardware (non-QEMU) firmware images,
# and the flash/monitor scripts with those store paths baked in:
#
#   nix run .#deploy -- start [/dev/ttyUSB0]   # flash a node + monitor
#   nix run .#monitor -- [/dev/ttyUSB0]        # serial monitor only
#
# The script sources live next to this file (deploy.bash, monitor.bash) with
# @espflash@/@firmware@ placeholders, substituted here and shellchecked by
# writeShellApplication.
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
