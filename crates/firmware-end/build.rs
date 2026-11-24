// Propagates ESP-IDF cfg flags and link arguments to the final binary.
fn main() {
    embuild::espidf::sysenv::output();
}
