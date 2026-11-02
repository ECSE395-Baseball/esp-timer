# Build both node firmwares in one derivation so the (expensive) ESP-IDF
# build is shared. With `qemu = true`, builds the emulated variant
# (raw-Ethernet link + virtual trigger) and additionally produces complete
# flash images ready for QEMU.
{
  lib,
  stdenv,
  ldproxy,
  espflash,
  git,
  zlib,
  libxml2,
  esp-idf,
  rust-xtensa,
  llvm-xtensa-lib,
  esp32-camera-src,
  cargoVendorDir,
  cargoSandboxSetup,
  PYTHONPATH_EXTRA,
}:

{
  qemu ? false,
}:

stdenv.mkDerivation {
  pname = "espnow-timer-firmware${if qemu then "-qemu" else ""}";
  version = "0.1.0";
  src = ./..;

  inherit cargoVendorDir;

  nativeBuildInputs = [
    rust-xtensa
    ldproxy
    espflash
    git
  ];
  # The esp-idf package's setup hook exports IDF_PATH and puts the
  # Xtensa GCC toolchain, cmake, ninja and the IDF python env on PATH.
  buildInputs = [ esp-idf ];

  # bindgen (used by esp-idf-sys) needs the Xtensa-aware libclang.
  LIBCLANG_PATH = "${llvm-xtensa-lib}/lib";
  ESP_IDF_VERSION = esp-idf.version;
  # The prebuilt libclang has no rpath entry for libz.
  LD_LIBRARY_PATH = lib.makeLibraryPath [
    zlib
    stdenv.cc.cc.lib
    libxml2
  ];

  dontUseCmakeConfigure = true;
  dontUseNinjaBuild = true;
  dontUseNinjaCheck = true;
  dontUseNinjaInstall = true;
  dontStrip = true;

  postUnpack = ''
    ${cargoSandboxSetup}

    # Let git operate on the root-owned ESP-IDF checkout in /nix/store.
    export GIT_CONFIG_COUNT=1
    export GIT_CONFIG_KEY_0=safe.directory
    export GIT_CONFIG_VALUE_0='*'

    # Extra python packages required by ESP-IDF 5.4's dep check.
    export PYTHONPATH="${PYTHONPATH_EXTRA}''${PYTHONPATH:+:$PYTHONPATH}"

    # Materialize the pinned esp32-camera source where the
    # submodule sits in a local checkout. Note: postUnpack runs
    # in the build directory, with the unpacked tree at
    # $sourceRoot (the cd happens after this hook).
    mkdir -p "$sourceRoot/crates/firmware-common/components"
    rm -rf "$sourceRoot/crates/firmware-common/components/esp32-camera"
    cp -r ${esp32-camera-src} "$sourceRoot/crates/firmware-common/components/esp32-camera"
    chmod -R u+w "$sourceRoot/crates/firmware-common/components/esp32-camera"
  '';

  buildPhase = ''
    runHook preBuild
    cargo build --release --offline -p firmware-start -p firmware-end ${lib.optionalString qemu "--features firmware-start/qemu,firmware-end/qemu"}
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out
    for fw in firmware-start firmware-end; do
      cp "target/xtensa-esp32-espidf/release/$fw" "$out/$fw.elf"
      espflash save-image --chip esp32 --flash-size 8mb "$out/$fw.elf" "$out/$fw.bin"
    done
    ${lib.optionalString qemu ''
      # Merge bootloader + partition table + app into complete flash
      # images for QEMU (8MB, matching the Feather ESP32 V2).
      build_dir=$(echo target/xtensa-esp32-espidf/release/build/esp-idf-sys-*/out/build)
      for fw in firmware-start firmware-end; do
        dd if=/dev/zero of="$out/qemu-$fw.bin" bs=1M count=8
        dd if="$build_dir/bootloader/bootloader.bin" of="$out/qemu-$fw.bin" bs=1 seek=$((0x1000)) conv=notrunc
        dd if="$build_dir/partition_table/partition-table.bin" of="$out/qemu-$fw.bin" bs=1 seek=$((0x8000)) conv=notrunc
        dd if="$out/$fw.bin" of="$out/qemu-$fw.bin" bs=1 seek=$((0x10000)) conv=notrunc
      done
    ''}
    runHook postInstall
  '';
}
