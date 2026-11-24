#!/usr/bin/env python3
"""Rebuild a camera sample frame from a captured ESP log.

The start node's camera task (crates/firmware-common/src/camera.rs,
FRAME_DUMP_PERIOD) periodically dumps a raw grayscale frame to the log
UART as base64 lines, wrapped in "frame dump BEGIN" / "frame dump END"
markers. Capture the monitor output (e.g. `espflash flash --monitor |
tee monitor.log`), then:

    python3 tools/grab_frame.py monitor.log frame.pgm

Uses the latest dump in the log. The output is a PGM image, viewable in
most image viewers, or convert to PNG with `convert frame.pgm frame.png`
(ImageMagick).
"""

import base64
import re
import sys


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(f"usage: {sys.argv[0]} <monitor log> <out.pgm>")

    dumps = []  # (width, height, base64 payload)
    w = h = None
    lines: list[str] = []
    in_dump = False
    with open(sys.argv[1], errors="replace") as f:
        for ln in f:
            if "frame dump BEGIN" in ln:
                m = re.search(r"(\d+)x(\d+)", ln)
                w, h = int(m.group(1)), int(m.group(2))
                lines, in_dump = [], True
            elif "frame dump END" in ln and in_dump:
                dumps.append((w, h, "".join(lines)))
                in_dump = False
            elif in_dump and "|" in ln:
                # Log line shape: "I (123) camera: |<payload>"
                lines.append(ln.split("|", 1)[1].strip())

    if not dumps:
        sys.exit("no frame dump found in the log")
    w, h, b64 = dumps[-1]
    raw = base64.b64decode(b64)
    if len(raw) != w * h:
        sys.exit(f"corrupt dump: got {len(raw)} B, expected {w * h} B")

    with open(sys.argv[2], "wb") as f:
        f.write(f"P5\n{w} {h}\n255\n".encode() + raw)
    print(f"wrote {sys.argv[2]}: {w}x{h}, {len(raw)} B")


if __name__ == "__main__":
    main()
