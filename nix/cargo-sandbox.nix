# Cargo/cargo-vendor setup shared by all sandboxed cargo builds.
{
  lib,
  rustPlatform,
  runCommand,
  rust-xtensa,
}:

let
  cargoVendorDir =
    let
      vendored = map (lockFile: rustPlatform.importCargoLock { inherit lockFile; }) [
        ../Cargo.lock
        "${rust-xtensa}/lib/rustlib/src/rust/library/Cargo.lock"
      ];
    in
    runCommand "cargo-vendor-merged" { } ''
      mkdir -p $out
      for dir in ${lib.concatStringsSep " " vendored}; do
        cp -rLn "$dir"/* $out/
      done
    '';
in
{
  inherit cargoVendorDir;

  cargoSandboxSetup = ''
    export HOME=$TMPDIR/home
    mkdir -p "$HOME"
    export CARGO_HOME=$TMPDIR/cargo-home
    mkdir -p "$CARGO_HOME"
    cat >"$CARGO_HOME/config.toml" <<EOF
    [source.crates-io]
    replace-with = "vendored-sources"
    [source.vendored-sources]
    directory = "$cargoVendorDir"
    EOF
  '';
}
