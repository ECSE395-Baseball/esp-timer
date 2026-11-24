# Writes the firmware images post-crane build to allow for reference stripping
{
  lib,
  runCommand,
  espflash,
  firmware,
  chip ? "esp32",
  qemu ? false,
}:

runCommand "espnow-timer-firmware-${chip}${lib.optionalString qemu "-qemu"}-images-0.1.0"
  {
    nativeBuildInputs = [ espflash ];
    # Stripping would rewrite the ELFs after espflash hashed them
    # (app_elf_sha256 in the image is the SHA-256 of the input ELF).
    dontStrip = true;
  }
  ''
    mkdir -p $out
    for fw in firmware-start firmware-end; do
      cp ${firmware}/$fw.elf "$out/$fw.elf"
      chmod u+w "$out/$fw.elf"
      espflash save-image --chip ${chip} --flash-size 8mb "$out/$fw.elf" "$out/$fw.bin"
    done
    ${lib.optionalString qemu ''
      # Merge bootloader + partition table + app into complete flash
      # images for QEMU (8MB, matching the Feather ESP32 V2).
      for fw in firmware-start firmware-end; do
        dd if=/dev/zero of="$out/qemu-$fw.bin" bs=1M count=8
        dd if=${firmware}/bootloader.bin of="$out/qemu-$fw.bin" bs=1 seek=$((0x1000)) conv=notrunc
        dd if=${firmware}/partition-table.bin of="$out/qemu-$fw.bin" bs=1 seek=$((0x8000)) conv=notrunc
        dd if="$out/$fw.bin" of="$out/qemu-$fw.bin" bs=1 seek=$((0x10000)) conv=notrunc
      done
    ''}
  ''
