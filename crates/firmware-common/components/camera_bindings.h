/* Bindings entry point for the vendored esp32-camera component.
 *
 * esp_camera.h cannot be used as bindgen's main file directly: it includes
 * img_converters.h, which includes esp_camera.h back, and `#pragma once` is
 * not honored for the main file — causing duplicate definitions. Including
 * it from here (not main file) makes the pragma effective again. */
#pragma once
#include "esp_camera.h"
