{
  description = "Two-part ESP-NOW timer for Adafruit Feather ESP32 V2 / ESP32-S3 (Rust on ESP-IDF)";

  inputs = {
    # Carry the esp-usb submodule (crates/firmware-common/components/) in
    # the flake source, so the sandboxed builds see the same checkout as
    # local devshell builds.
    self.submodules = true;

    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    # ESP-IDF SDK packaging + Xtensa GCC toolchain + Xtensa libclang.
    # (Its pinned ESP-IDF is v5.3.2; we rebuild the same packaging with a
    # newer ESP-IDF below.)
    esp-flake.url = "github:milas/esp-flake";
    # Incremental cargo builds in the sandbox (deps-only/workspace split).
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { ... }@inputs:
    # The Espressif Rust toolchain is only published for x86_64-linux.
    inputs.flake-utils.lib.eachSystem [ "x86_64-linux" ] (
      system:
      let
        pkgs = import inputs.nixpkgs { inherit system; };
        # milas/esp-flake pins ESP-IDF v5.3.2, but esp-idf-svc 0.53 needs
        # >= 5.4, so rebuild the same packaging with a newer ESP-IDF.
        # Built against esp-flake's own pinned nixpkgs, like upstream does.
        esp-flake-nixpkgs = import inputs.esp-flake.inputs.nixpkgs { inherit system; };
        esp-idf = esp-flake-nixpkgs.callPackage "${inputs.esp-flake}/pkgs/esp-idf" {
          rev = "v5.4.4";
          sha256 = "sha256-Vf4WG4LyljMOfWjt1VqWqIlwzzsf/vD+XI9gNFSSM9U=";
          toolsToInclude = [
            "xtensa-esp-elf"
            "xtensa-esp-elf-gdb"
            "esp32ulp-elf"
            "openocd-esp32"
          ];
        };
        llvm-xtensa-lib = inputs.esp-flake.packages.${system}.llvm-xtensa-lib;

        # ESP-IDF 5.4's python dependency check additionally wants psutil,
        # tree_sitter and tree_sitter_c, which esp-flake's python env (pinned
        # for 5.3) lacks. Expose them via PYTHONPATH; same python3 package
        # set as the ESP-IDF env, so the C extensions are ABI-compatible.
        esp-python-extra = esp-flake-nixpkgs.python3.withPackages (ps: [
          ps.psutil
          ps.tree-sitter
          (ps.callPackage ./nix/tree-sitter-c.nix {
            tree-sitter-headers = ps.tree-sitter.src;
          })
        ]);
        inherit (esp-flake-nixpkgs.python3) sitePackages;
        PYTHONPATH_EXTRA = "${esp-python-extra}/${sitePackages}";

        rust-xtensa = pkgs.callPackage ./nix/rust-xtensa.nix { };
        qemu-esp32 = pkgs.callPackage ./nix/qemu-esp32.nix { };

        # Crane bound to the Espressif Rust fork: splits the sandboxed
        # cargo builds into a deps-only derivation (all dependencies,
        # including the ESP-IDF cmake build via esp-idf-sys) and a
        # workspace derivation, so editing workspace sources no longer
        # recompiles all dependencies.
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (_p: rust-xtensa);

        # Vendored cargo deps shared by all sandboxed cargo builds: the
        # workspace lock plus the rust library lock (build-std compiles
        # std from rust-src, so its deps must be vendored too).
        cargoVendorDir = craneLib.vendorMultipleCargoDeps {
          cargoLockList = [
            ./Cargo.lock
            "${rust-xtensa}/lib/rustlib/src/rust/library/Cargo.lock"
          ];
        };

        # Reproducible name ("source") regardless of the checkout's
        # directory name; keeps the build path stable between crane's
        # deps-only and workspace derivations.
        src = craneLib.path ./.;

        treefmtconfig = inputs.treefmt-nix.lib.evalModule pkgs {
          projectRootFile = "flake.nix";
          programs = {
            nixfmt.enable = true;
            rustfmt = {
              enable = true;
              edition = "2021";
            };
            shfmt.enable = true;
            shellcheck.enable = true;
            taplo.enable = true;
            mdformat = {
              enable = true;
              plugins = ps: [
                ps.mdformat-gfm
                ps.mdformat-frontmatter
              ];
              settings = {
                wrap = 88;
                end-of-line = "lf";
              };
            };
          };
          settings = {
            formatter.shellcheck.excludes = [ ".envrc" ];
            # The component submodules are third-party code; don't format/check them.
            global.excludes = [
              "crates/firmware-common/components/esp-usb/**"
              "crates/firmware-common/components/esp32-camera/**"
            ];
          };
        };

        # Build both node firmwares in one derivation so the (expensive)
        # ESP-IDF build is shared. `chip` selects the MCU ("esp32" =
        # Feather ESP32 V2, "esp32s3" = Feather ESP32-S3). With
        # `qemu = true`, builds the emulated variant (raw-Ethernet link +
        # virtual trigger, always esp32). This produces only the ELFs;
        # the checksummed app/flash images are generated one derivation
        # later by mkFirmwareImages (see nix/firmware-images.nix for why).
        mkFirmware = pkgs.callPackage ./nix/firmware.nix {
          inherit
            esp-idf
            rust-xtensa
            llvm-xtensa-lib
            craneLib
            cargoVendorDir
            src
            PYTHONPATH_EXTRA
            ;
        };

        # Turn the ELFs into flashable images: the raw app image per node
        # and, for the qemu variant, complete flash images ready for QEMU.
        mkFirmwareImages =
          args:
          pkgs.callPackage ./nix/firmware-images.nix {
            firmware = mkFirmware args;
            chip = args.chip or "esp32";
            qemu = args.qemu or false;
          };

        firmwareArgs = {
          "default" = { };
          "s3" = {
            chip = "esp32s3";
          };
          "qemu" = {
            qemu = true;
          };
          "camera" = {
            camera = true;
          };
          "s3-camera" = {
            chip = "esp32s3";
            camera = true;
          };
          "s3-tft" = {
            chip = "esp32s3";
            tftDisplay = true;
          };
          "s3-tft-camera" = {
            chip = "esp32s3";
            camera = true;
            tftDisplay = true;
          };
        };

        # firmware and deploy packages with an argument map
        firmware = pkgs.lib.mapAttrs (name: value: mkFirmwareImages value) firmwareArgs;
        deploy = pkgs.lib.mapAttrs (name: value: pkgs.callPackage ./nix/deploy.nix { firmware = value; }) (
          pkgs.lib.filterAttrs (name: value: name != "qemu") firmware
        );

        # Host-side channel simulator used to wire QEMU nodes together with
        # simulated delay/jitter/loss. Built for the host, not the ESP32.
        channel-sim = pkgs.callPackage ./nix/channel-sim.nix {
          inherit craneLib cargoVendorDir src;
        };

        # QEMU simulation bundle: QEMU + firmware flash images + channel-sim
        # + the runner scripts with all store paths baked in.
        qemu-sim = pkgs.callPackage ./nix/qemu-sim.nix {
          inherit qemu-esp32 channel-sim;
          firmware-qemu = firmware.qemu;
        };
      in
      {
        packages = {
          default = firmware;
          inherit
            rust-xtensa
            esp-idf
            channel-sim
            qemu-esp32
            qemu-sim
            ;
          inherit (pkgs) espflash rust-analyzer;
        }
        // (pkgs.lib.mapAttrs' (name: value: pkgs.lib.nameValuePair "firmware-${name}" value) firmware)
        // (pkgs.lib.mapAttrs' (name: value: pkgs.lib.nameValuePair "deploy-${name}" value) deploy);

        apps = {
          qemu-sim = {
            type = "app";
            program = "${qemu-sim}/bin/qemu-sim";
          };
          qemu-run = {
            type = "app";
            program = "${qemu-sim}/bin/qemu-run";
          };
          monitor = {
            type = "app";
            program = "${deploy}/bin/monitor";
          };
        }
        // pkgs.lib.mapAttrs' (
          name: value:
          pkgs.lib.nameValuePair "deploy-${name}" {
            type = "app";
            program = "${value}/bin/deploy";
          }
        ) deploy;

        devShells.default = pkgs.mkShell {
          name = "espnow-timer";

          packages =
            with pkgs;
            [
              ldproxy
              espflash
              git
              nil
              nixd
              rust-analyzer
            ]
            ++ [
              rust-xtensa
              qemu-esp32
            ];
          # IDF_PATH and the ESP-IDF tools/python env come from this hook.
          buildInputs = [ esp-idf ];

          shellHook = ''
            # bindgen needs the Xtensa-aware libclang.
            export LIBCLANG_PATH=${llvm-xtensa-lib}/lib
            export ESP_IDF_VERSION=${esp-idf.version}
            # Dynamic library workaround for the prebuilt Espressif binaries.
            export LD_LIBRARY_PATH=${
              with pkgs;
              lib.makeLibraryPath [
                libxml2
                zlib
                stdenv.cc.cc.lib
              ]
            }
            # Let git operate on the root-owned ESP-IDF checkout in /nix/store.
            export GIT_CONFIG_COUNT=1
            export GIT_CONFIG_KEY_0=safe.directory
            export GIT_CONFIG_VALUE_0='*'
            # Extra python packages required by ESP-IDF 5.4's dep check.
            export PYTHONPATH="${PYTHONPATH_EXTRA}''${PYTHONPATH:+:$PYTHONPATH}"
          '';
        };

        formatter = treefmtconfig.config.build.wrapper;
        checks.formatting = treefmtconfig.config.build.check inputs.self;
      }
    );
}
