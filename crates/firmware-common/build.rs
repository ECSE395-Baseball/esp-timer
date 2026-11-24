// Propagates ESP-IDF cfg flags (chip cfgs like `esp32s3`) and link
// arguments from esp-idf-sys; a no-op on non-ESP builds.
fn main() {
    embuild::espidf::sysenv::output();
}
