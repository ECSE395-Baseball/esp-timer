# USB HID host class driver (espressif/usb_host_hid), vendored as the
# esp-usb git submodule at components/esp-usb
if(IDF_TARGET MATCHES "^(esp32s2|esp32s3|esp32s31|esp32p4|esp32h4)$")
    set(hid_dir "${CMAKE_CURRENT_SOURCE_DIR}/../esp-usb/host/class/hid/usb_host_hid")
    idf_component_register(SRCS "${hid_dir}/hid_host.c"
                           INCLUDE_DIRS "${hid_dir}/include"
                           REQUIRES usb)
else()
    idf_component_register()
endif()
