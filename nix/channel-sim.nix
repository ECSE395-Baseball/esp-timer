# Host-side channel simulator used to wire QEMU nodes together with
# simulated delay/jitter/loss. Built for the host, not the ESP32.
{
  stdenv,
  rust-xtensa,
  cargoVendorDir,
  cargoSandboxSetup,
}:

stdenv.mkDerivation {
  pname = "channel-sim";
  version = "0.1.0";
  src = ./..;

  inherit cargoVendorDir;

  nativeBuildInputs = [ rust-xtensa ];

  postUnpack = cargoSandboxSetup;

  buildPhase = ''
    runHook preBuild
    cargo build --release --offline -p channel-sim \
      --target x86_64-unknown-linux-gnu \
      --config 'unstable.build-std=[]'
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 target/x86_64-unknown-linux-gnu/release/channel-sim $out/bin/channel-sim
    runHook postInstall
  '';
}
