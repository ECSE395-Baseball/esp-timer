# Build both node firmwares in one derivation so the (expensive) ESP-IDF
# build is shared.
#
# Two staged to preserve dependencies across minimal change rebuilds
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
  craneLib,
  cargoVendorDir,
  src,
  PYTHONPATH_EXTRA,
}:

{
  qemu ? false,
  chip ? "esp32",
  camera ? false,
  tftDisplay ? false,
}:

# The Espressif QEMU fork used here only emulates the original ESP32.
assert lib.assertMsg (
  qemu -> chip == "esp32"
) "qemu firmware is only supported for chip = \"esp32\"";

# The onboard ST7789 exists only on the Feather TFT ESP32-S3.
assert lib.assertMsg (
  tftDisplay -> chip == "esp32s3"
) "tftDisplay firmware is only supported for chip = \"esp32s3\"";

let
  cargoTarget =
    {
      esp32 = "xtensa-esp32-espidf";
      esp32s3 = "xtensa-esp32s3-espidf";
    }
    .${chip};

  featureList =
    lib.optional qemu "firmware-start/qemu"
    ++ lib.optional qemu "firmware-end/qemu"
    ++ lib.optional camera "firmware-start/camera"
    ++ lib.optional tftDisplay "firmware-start/tft-display";
  featuresArg = lib.optionalString (
    featureList != [ ]
  ) " --features ${lib.concatStringsSep "," featureList}";

  firmwareComponents = lib.cleanSourceWith {
    src = ../crates/firmware-common/components;
    filter = lib.cleanSourceFilter;
  };

  commonArgs = {
    inherit src cargoVendorDir;

    pname = "espnow-timer-firmware-${chip}${lib.optionalString qemu "-qemu"}${lib.optionalString camera "-camera"}${lib.optionalString tftDisplay "-tft"}";
    version = "0.1.0";

    # Chip selection. These override .cargo/config.toml's defaults:
    MCU = chip;
    CARGO_BUILD_TARGET = cargoTarget;

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

    # Tests are not run in the sandbox (and must not be compiled either).
    doCheck = false;

    cargoExtraArgs = "--locked --offline -p firmware-start -p firmware-end${featuresArg}";

    postUnpack = ''
      export HOME=/tmp/esp-timer-home
      mkdir -p "$HOME"

      # Let git operate on the root-owned ESP-IDF checkout in /nix/store.
      export GIT_CONFIG_COUNT=1
      export GIT_CONFIG_KEY_0=safe.directory
      export GIT_CONFIG_VALUE_0='*'

      # Extra python packages required by ESP-IDF 5.4's dep check.
      export PYTHONPATH="${PYTHONPATH_EXTRA}''${PYTHONPATH:+:$PYTHONPATH}"
    '';
  };

  # The dummy workspace sources for the deps-only build.
  dummySrc = craneLib.mkDummySrc {
    inherit src;
    cleanCargoTomlFilter =
      path:
      (craneLib.filters.cargoTomlDefault path)
      || (
        lib.take 2 path == [
          "package"
          "metadata"
        ]
      );
    # Proxy build.rs
    extraDummyScript = ''
      cp ${../sdkconfig.defaults} $out/sdkconfig.defaults
      mkdir -p $out/crates/firmware-common
      cp -r ${firmwareComponents} $out/crates/firmware-common/components
      chmod -R u+w $out/crates/firmware-common/components

      for fw in firmware-start firmware-end; do
        rm -f "$out/crates/$fw/src/main.rs"
        cat > "$out/crates/$fw/src/main.rs" <<'EOF'
      #![allow(unused)]
      extern crate esp_idf_svc;
      fn main() {}
      EOF
      done
      for fw in firmware-start firmware-end firmware-common; do
        chmod u+w "$out/crates/$fw/Cargo.toml"
        sed -i 's|^build = "/nix/store/[a-z0-9]*-dummyBuild.rs"$|build = "build.rs"|' "$out/crates/$fw/Cargo.toml"
        grep -q '^build = "build.rs"$' "$out/crates/$fw/Cargo.toml"
        cat > "$out/crates/$fw/build.rs" <<'EOF'
      fn main() {
          embuild::espidf::sysenv::output();
      }
      EOF
      done
    '';
  };

  # All cargo dependencies, incl. the ESP-IDF cmake build
  cargoArtifacts = craneLib.buildDepsOnly (
    (removeAttrs commonArgs [ "src" ])
    // {
      inherit dummySrc;
      cargoCheckCommand = "true";
    }
  );
in

craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    installPhaseCommand = ''
      mkdir -p $out
      for fw in firmware-start firmware-end; do
        cp "target/${cargoTarget}/release/$fw" "$out/$fw.elf"
      done
      ${lib.optionalString qemu ''
        build_dir=$(echo target/${cargoTarget}/release/build/esp-idf-sys-*/out/build)
        cp "$build_dir/bootloader/bootloader.bin" "$out/bootloader.bin"
        cp "$build_dir/partition_table/partition-table.bin" "$out/partition-table.bin"
      ''}
    '';
  }
)
