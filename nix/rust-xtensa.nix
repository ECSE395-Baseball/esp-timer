# Espressif's Rust fork with Xtensa target support (the toolchain normally
# installed by `espup`), packaged as a plain Nix derivation.
{
  stdenv,
  fetchurl,
  autoPatchelfHook,
  zlib,
}:

let
  version = "1.97.0.0";
  rustInstaller = fetchurl {
    url = "https://github.com/esp-rs/rust-build/releases/download/v${version}/rust-${version}-x86_64-unknown-linux-gnu.tar.xz";
    hash = "sha256-qZv+5pIh6f9thjiPaBHuaIzUBeagQAo80XhOjUY+nZk=";
  };
  rustSrc = fetchurl {
    url = "https://github.com/esp-rs/rust-build/releases/download/v${version}/rust-src-${version}.tar.xz";
    hash = "sha256-Vo1oi5+PMy7E0EZXVE+tI+mc4R6ebPWDWXnmiijGi3M=";
  };
in
stdenv.mkDerivation {
  pname = "rust-xtensa";
  inherit version;

  dontUnpack = true;
  dontStrip = true;

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    stdenv.cc.cc.lib
    zlib
  ];

  installPhase = ''
    runHook preInstall

    mkdir rust rust-src
    tar -xf ${rustInstaller} -C rust --strip-components=1
    tar -xf ${rustSrc} -C rust-src --strip-components=1

    sh ./rust/install.sh --prefix=$out --disable-ldconfig
    sh ./rust-src/install.sh --prefix=$out

    runHook postInstall
  '';

  meta = {
    description = "Espressif Rust toolchain fork with Xtensa (ESP32) support";
    homepage = "https://github.com/esp-rs/rust-build";
    platforms = [ "x86_64-linux" ];
  };
}
