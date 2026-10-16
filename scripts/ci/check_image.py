"""Fails unless a BMP written by `voxferro --screenshot` holds a real image.

Usage: python scripts/ci/check_image.py out.bmp [--stats]

With --stats it only prints the numbers (for diagnostic images, which are
meant to be uniform or bright).

A smoke test, not a comparison: rendering on a software GPU in CI differs
from any reference, but a blank, black or uniform frame means something
did not draw. Checks the size, that the mean is not at either end, and that
the pixels vary."""
import struct, sys

path = sys.argv[1]
stats_only = "--stats" in sys.argv[2:]
data = open(path, "rb").read()
if data[:2] != b"BM":
    sys.exit(f"{path}: not a BMP")
offset = struct.unpack_from("<I", data, 10)[0]
width, height = struct.unpack_from("<ii", data, 18)
bpp = struct.unpack_from("<H", data, 28)[0] // 8
height = abs(height)
stride = (width * bpp + 3) // 4 * 4
if width < 64 or height < 64:
    sys.exit(f"{path}: {width}x{height} is too small")

values = []
for y in range(0, height, 4):
    row = offset + y * stride
    for x in range(0, width, 4):
        b, g, r = data[row + x * bpp : row + x * bpp + 3]
        values.append((r + g + b) / 3)
mean = sum(values) / len(values)
spread = (sum((v - mean) ** 2 for v in values) / len(values)) ** 0.5
print(f"{path}: {width}x{height}, mean {mean:.1f}, spread {spread:.1f}")
if stats_only:
    sys.exit(0)
if not 10 < mean < 245:
    sys.exit("the image is nearly black or white")
if spread < 8:
    sys.exit("the image is nearly uniform")
