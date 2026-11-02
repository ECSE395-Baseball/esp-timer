{
  description = "Two-part ESP-NOW timer for Adafruit Feather ESP32 V2 (Rust on ESP-IDF)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    # ESP-IDF SDK packaging + Xtensa GCC toolchain + Xtensa libclang.
    # (Its pinned ESP-IDF is v5.3.2; we rebuild the same packaging with a
    # newer ESP-IDF below.)
    esp-flake.url = "github:milas/esp-flake";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      treefmt-nix,
      esp-flake,
    }:
    # The Espressif Rust toolchain is only published for x86_64-linux.
    flake-utils.lib.eachSystem [ "x86_64-linux" ] (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        # milas/esp-flake pins ESP-IDF v5.3.2, but esp-idf-svc 0.53 needs
        # >= 5.4, so rebuild the same packaging with a newer ESP-IDF.
        # Built against esp-flake's own pinned nixpkgs, like upstream does.
        esp-flake-nixpkgs = import esp-flake.inputs.nixpkgs { inherit system; };
        esp-idf = esp-flake-nixpkgs.callPackage "${esp-flake}/pkgs/esp-idf" {
          rev = "v5.4.4";
          sha256 = "sha256-Vf4WG4LyljMOfWjt1VqWqIlwzzsf/vD+XI9gNFSSM9U=";
          toolsToInclude = [
            "xtensa-esp-elf"
            "xtensa-esp-elf-gdb"
            "esp32ulp-elf"
            "openocd-esp32"
          ];
        };
        llvm-xtensa-lib = esp-flake.packages.${system}.llvm-xtensa-lib;

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

        # esp32-camera is a git submodule (crates/firmware-common/components/)
        # for local devshell builds. For the sandboxed build, materialize the
        # pinned source instead, so the build never depends on submodule
        # checkout state.
        esp32-camera-src = pkgs.callPackage ./nix/esp32-camera-src.nix { };

        treefmtconfig = treefmt-nix.lib.evalModule pkgs {
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
          settings.formatter.shellcheck.excludes = [ ".envrc" ];
        };

        # Cargo vendor dir + offline setup shared by the sandboxed cargo
        # builds (firmware.nix and channel-sim.nix).
        inherit (pkgs.callPackage ./nix/cargo-sandbox.nix { inherit rust-xtensa; })
          cargoVendorDir
          cargoSandboxSetup
          ;

        # Build both node firmwares in one derivation so the (expensive)
        # ESP-IDF build is shared. With `qemu = true`, builds the emulated
        # variant (raw-Ethernet link + virtual trigger) and additionally
        # produces complete flash images ready for QEMU.
        mkFirmware = pkgs.callPackage ./nix/firmware.nix {
          inherit
            esp-idf
            rust-xtensa
            llvm-xtensa-lib
            esp32-camera-src
            cargoVendorDir
            cargoSandboxSetup
            PYTHONPATH_EXTRA
            ;
        };

        firmware = mkFirmware { };
        firmware-qemu = mkFirmware { qemu = true; };

        firmwareFor =
          role: qemu:
          pkgs.runCommand "firmware-${role}${if qemu then "-qemu" else ""}" { } ''
            mkdir -p $out
            src=${if qemu then firmware-qemu else firmware}
            ln -s "$src/firmware-${role}.elf" $out/firmware-${role}.elf
            ln -s "$src/firmware-${role}.bin" $out/firmware-${role}.bin
            ${pkgs.lib.optionalString qemu ''
              ln -s "$src/qemu-firmware-${role}.bin" $out/qemu-firmware-${role}.bin
            ''}
          '';

        # Host-side channel simulator used to wire QEMU nodes together with
        # simulated delay/jitter/loss. Built for the host, not the ESP32.
        channel-sim = pkgs.callPackage ./nix/channel-sim.nix {
          inherit rust-xtensa cargoVendorDir cargoSandboxSetup;
        };

        # QEMU simulation bundle: QEMU + firmware flash images + channel-sim
        # + the runner scripts with all store paths baked in.
        qemu-sim = pkgs.callPackage ./nix/qemu-sim.nix {
          inherit qemu-esp32 firmware-qemu channel-sim;
        };

        # Hardware deploy bundle: espflash + the hardware firmware images +
        # the flash/monitor scripts with all store paths baked in.
        deploy = pkgs.callPackage ./nix/deploy.nix {
          inherit firmware;
        };
      in
      {
        packages = {
          default = firmware;
          inherit
            firmware
            firmware-qemu
            rust-xtensa
            esp-idf
            channel-sim
            qemu-esp32
            qemu-sim
            deploy
            ;
          firmware-start = firmwareFor "start" false;
          firmware-end = firmwareFor "end" false;
          firmware-start-qemu = firmwareFor "start" true;
          firmware-end-qemu = firmwareFor "end" true;
          inherit (pkgs) espflash rust-analyzer;
        };

        apps = {
          qemu-sim = {
            type = "app";
            program = "${qemu-sim}/bin/qemu-sim";
          };
          qemu-run = {
            type = "app";
            program = "${qemu-sim}/bin/qemu-run";
          };
          deploy = {
            type = "app";
            program = "${deploy}/bin/deploy";
          };
          monitor = {
            type = "app";
            program = "${deploy}/bin/monitor";
          };
        };

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
        checks.formatting = treefmtconfig.config.build.check self;
      }
    );
}
