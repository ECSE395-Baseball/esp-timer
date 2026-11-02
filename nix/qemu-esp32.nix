# Espressif's QEMU fork (prebuilt binaries), which emulates the ESP32.
# WiFi is NOT emulated; Ethernet (OpenCores MAC) is, which the firmware's
# `qemu` feature uses as the link layer.
{
  stdenv,
  fetchurl,
  autoPatchelfHook,
  glib,
  libgcrypt,
  pixman,
  SDL2,
  libslirp,
  zlib,
}:

let
  qemuVersion = "9.2.2";
  qemuDate = "20260417";
in
stdenv.mkDerivation {
  pname = "qemu-esp32";
  version = "${qemuVersion}-${qemuDate}";

  src = fetchurl {
    url = "https://github.com/espressif/qemu/releases/download/esp-develop-${qemuVersion}-${qemuDate}/qemu-xtensa-softmmu-esp_develop_${qemuVersion}_${qemuDate}-x86_64-linux-gnu.tar.xz";
    hash = "sha256-Duyyo0pVhsDlkRD3e5NDt7M26C/bDhow4dwbq4pUfjU=";
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    glib
    libgcrypt
    pixman
    SDL2
    libslirp
    zlib
    stdenv.cc.cc.lib
  ];

  sourceRoot = ".";
  dontBuild = true;
  dontConfigure = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp -r qemu/* $out/
    runHook postInstall
  '';

  meta = {
    description = "QEMU fork with ESP32 support (Espressif)";
    homepage = "https://github.com/espressif/qemu";
    platforms = [ "x86_64-linux" ];
  };
}
