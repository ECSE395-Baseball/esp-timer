# Host-side channel simulator used to wire QEMU nodes together with
# simulated delay/jitter/loss. Built for the host, not the ESP32.
{
  craneLib,
  cargoVendorDir,
  src,
}:

let
  commonArgs = {
    inherit src cargoVendorDir;

    pname = "channel-sim";
    version = "0.1.0";

    doCheck = false;

    cargoExtraArgs = "--locked --offline -p channel-sim --target x86_64-unknown-linux-gnu --config 'unstable.build-std=[]'";
  };

  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      # Skip the `cargo check` pass: only `cargo build` artifacts are used.
      cargoCheckCommand = "true";
    }
  );
in

craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    installPhaseCommand = ''
      install -Dm755 target/x86_64-unknown-linux-gnu/release/channel-sim $out/bin/channel-sim
    '';
  }
)
