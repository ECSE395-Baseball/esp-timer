/* Bindings entry point for the vendored usb_host_hid component (bindings
 * module `usbhid`).
 *
 * The in-tree `usb` host library only publishes its headers on targets
 * with a USB-OTG peripheral (esp32s2/s3/...); on plain esp32 (QEMU) its
 * include path is absent and the wrapper component registers no include
 * dirs either, so guard on presence and generate an empty module there.
 * The Rust side only touches these bindings under cfg(esp32s3). */
#pragma once
#if __has_include("usb/usb_host.h") && __has_include("usb/hid_host.h")
#include "usb/usb_host.h"
#include "usb/hid_host.h"
#endif
