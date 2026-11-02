# Python binding for the tree-sitter C grammar, needed by ESP-IDF >= 5.4's
# python dependency check (used by the NimBLE compressed-log tooling, not by
# the firmware build itself).
{
  lib,
  buildPythonPackage,
  fetchPypi,
  setuptools,
  # Source of the py-tree-sitter package; its vendored tree-sitter core has
  # the parser.h header the grammar's parser.c needs (not installed by the
  # nixpkgs tree-sitter package).
  tree-sitter-headers,
}:

buildPythonPackage rec {
  pname = "tree-sitter-c";
  # Matches the tree-sitter core vendored in py-tree-sitter 0.23.0
  # (newer grammars use TSLanguage fields its parser.h lacks).
  version = "0.23.0";

  pyproject = true;
  build-system = [ setuptools ];

  preBuild = ''
    mkdir -p .ts-headers/tree_sitter
    ln -s ${tree-sitter-headers}/tree_sitter/core/lib/src/parser.h .ts-headers/tree_sitter/parser.h
    export NIX_CFLAGS_COMPILE="''${NIX_CFLAGS_COMPILE:-} -I$PWD/.ts-headers"
  '';

  src = fetchPypi {
    pname = "tree_sitter_c";
    inherit version;
    hash = "sha256-DAjwXqJM5nAtXqgyBPBsDVssKEcFA+vuYm56hz/gay8=";
  };

  # Building the bundled C grammar needs no extra inputs; skip the test
  # suite (it pulls in the tree-sitter CLI).
  doCheck = false;

  meta = {
    description = "C grammar for tree-sitter";
    homepage = "https://github.com/tree-sitter/tree-sitter-c";
    license = lib.licenses.mit;
  };
}
