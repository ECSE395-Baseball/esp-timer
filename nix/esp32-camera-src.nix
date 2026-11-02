# Pinned esp32-camera source for sandboxed builds.
{ fetchFromGitHub }:

fetchFromGitHub {
  owner = "espressif";
  repo = "esp32-camera";
  # v2.0.15 is the last release that does not depend on the
  # espressif/esp_jpeg component
  rev = "v2.0.15";
  hash = "sha256-yO7zHJocXCLugqMEct/u+lkcLERfolJ6eRuYCGi3+zI=";
}
