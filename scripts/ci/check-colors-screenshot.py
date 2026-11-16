#!/usr/bin/env python3
"""Check a screenshot of tests/pages/colors.html.

    scripts/ci/check-colors-screenshot.py <screenshot.png>

The page is white with a 160x160 CSS-pixel red square at the top left, a blue
one at the top right, an 80 CSS-pixel green bar along the bottom and a black
heading 200 CSS pixels from the top. The screenshot is of the whole window, so
the browser's own tab strip and toolbar sit above the page; their height is
not assumed. The checker finds the top of the page by looking for the red
square's top edge, measures the square to learn how many image pixels make a
CSS pixel (so any display scale and window size work), and then samples well
inside every region: red, blue, green, and white in the gaps between them.

Prints one line per check. Exit status: 0 when every check passes, 1 when the
picture is wrong, 2 when the file is not a usable PNG (truncated, corrupt,
malformed, larger than a screenshot can be, or in a form this script does not
read). Standard library only.
"""

import struct
import sys
import zlib

# Geometry of tests/pages/colors.html, in CSS pixels.
SQUARE = 160
BAR = 80
HEADING_TOP = 200
HEADING_BOTTOM = 260  # 40px bold heading; its line box ends well before this

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# Limits well above any screenshot (a 900x700 window at 3x is 2700x2100), so a
# small crafted file cannot make the decoder allocate without bound.
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_SIDE = 16384
MAX_PIXELS = 40 * 1000 * 1000


class PngError(Exception):
    pass


class Image:
    def __init__(self, width, height, channels, rows):
        self.width = width
        self.height = height
        self.channels = channels
        self.rows = rows

    def pixel(self, x, y):
        """(r, g, b) at integer image coordinates."""
        row = self.rows[y]
        offset = x * self.channels
        if self.channels in (1, 2):
            value = row[offset]
            return (value, value, value)
        return (row[offset], row[offset + 1], row[offset + 2])

    def average(self, x, y, radius=1):
        """Mean (r, g, b) of the (2*radius+1)^2 block centred on (x, y), clipped to the image."""
        totals = [0, 0, 0]
        count = 0
        for yy in range(max(0, y - radius), min(self.height, y + radius + 1)):
            for xx in range(max(0, x - radius), min(self.width, x + radius + 1)):
                for channel, value in enumerate(self.pixel(xx, yy)):
                    totals[channel] += value
                count += 1
        return tuple(total // count for total in totals)


def _paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def decode_png(data):
    """Decode an 8-bit, non-interlaced greyscale, RGB, palette or RGBA PNG."""
    if not data.startswith(PNG_SIGNATURE):
        raise PngError("not a PNG file (bad signature)")
    position = len(PNG_SIGNATURE)
    header = None
    palette = None
    compressed = []
    seen_end = False
    while position < len(data):
        if position + 8 > len(data):
            raise PngError("truncated chunk header at byte %d" % position)
        length, kind = struct.unpack(">I4s", data[position : position + 8])
        body_start = position + 8
        body_end = body_start + length
        if body_end + 4 > len(data):
            raise PngError("truncated %s chunk at byte %d" % (kind.decode("latin-1"), position))
        body = data[body_start:body_end]
        (crc,) = struct.unpack(">I", data[body_end : body_end + 4])
        if zlib.crc32(kind + body) & 0xFFFFFFFF != crc:
            raise PngError("CRC mismatch in %s chunk" % kind.decode("latin-1"))
        position = body_end + 4
        if header is None and kind != b"IHDR":
            raise PngError("first chunk is not IHDR")
        if kind == b"IHDR":
            if header is not None:
                raise PngError("more than one IHDR chunk")
            if length != 13:
                raise PngError("IHDR chunk is %d bytes long, not 13" % length)
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"PLTE":
            if palette is not None:
                raise PngError("more than one PLTE chunk")
            if length == 0 or length % 3 != 0 or length > 3 * 256:
                raise PngError("PLTE chunk is %d bytes long, not a multiple of 3 up to 768" % length)
            palette = [tuple(body[i : i + 3]) for i in range(0, len(body), 3)]
        elif kind == b"IDAT":
            compressed.append(body)
        elif kind == b"IEND":
            seen_end = True
            break
    if header is None:
        raise PngError("no IHDR chunk")
    if not seen_end:
        raise PngError("no IEND chunk (file truncated?)")
    width, height, depth, colour_type, compression, filter_method, interlace = header
    channels_by_type = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
    if colour_type not in channels_by_type:
        raise PngError("unsupported colour type %d" % colour_type)
    if depth != 8:
        raise PngError("unsupported bit depth %d (only 8 is handled)" % depth)
    if compression != 0 or filter_method != 0:
        raise PngError("unknown compression or filter method in IHDR")
    if interlace != 0:
        raise PngError("interlaced PNGs are not handled")
    if colour_type == 3 and not palette:
        raise PngError("palette image without a PLTE chunk")
    if width == 0 or height == 0:
        raise PngError("empty image")
    if width > MAX_SIDE or height > MAX_SIDE or width * height > MAX_PIXELS:
        raise PngError("%dx%d is larger than any screenshot this checks" % (width, height))

    channels = channels_by_type[colour_type]
    stride = width * channels
    expected = height * (stride + 1)

    # Never produce more than the header allows: max_length stops a small
    # stream that would expand without limit, and anything left over is an error.
    decompressor = zlib.decompressobj()
    try:
        raw = decompressor.decompress(b"".join(compressed), expected)
        overflow = bool(decompressor.unconsumed_tail) or (
            not decompressor.eof and bool(decompressor.decompress(b"", 1))
        )
    except zlib.error as error:
        raise PngError("image data does not decompress: %s" % error)
    if overflow:
        raise PngError("image data is larger than the %dx%d header allows" % (width, height))
    if not decompressor.eof:
        raise PngError("image data stream is incomplete")
    if len(raw) != expected:
        raise PngError("image data has %d bytes, expected %d" % (len(raw), expected))

    rows = []
    previous = bytearray(stride)
    bpp = channels
    for y in range(height):
        start = y * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1 : start + 1 + stride])
        if kind == 0:
            pass
        elif kind == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif kind == 2:
            for i in range(stride):
                line[i] = (line[i] + previous[i]) & 0xFF
        elif kind == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + previous[i]) >> 1)) & 0xFF
        elif kind == 4:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                upper_left = previous[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + _paeth(left, previous[i], upper_left)) & 0xFF
        else:
            raise PngError("unknown filter type %d on row %d" % (kind, y))
        rows.append(line)
        previous = line

    if colour_type == 3:
        expanded = []
        for line in rows:
            out = bytearray()
            for index in line:
                if index >= len(palette):
                    raise PngError("palette index out of range")
                out.extend(palette[index])
            expanded.append(out)
        return Image(width, height, 3, expanded)
    return Image(width, height, channels, rows)


def classify(rgb):
    r, g, b = rgb
    if r >= 235 and g >= 235 and b >= 235:
        return "white"
    if r >= 200 and g <= 60 and b <= 60:
        return "red"
    if g >= 200 and r <= 60 and b <= 60:
        return "green"
    if b >= 200 and r <= 60 and g <= 60:
        return "blue"
    return "other"


def run_length(image, x, y, dx, dy, colour):
    """Number of consecutive pixels of `colour` starting at (x, y) and stepping by (dx, dy)."""
    count = 0
    while 0 <= x < image.width and 0 <= y < image.height and classify(image.pixel(x, y)) == colour:
        count += 1
        x += dx
        y += dy
    return count


def check(image):
    results = []

    def record(name, ok, detail):
        results.append((name, ok, detail))
        return ok

    # 1. The top edge of the red square is the top of the page.
    probe_x = max(1, image.width // 100)
    top = None
    for y in range(image.height // 2):
        if classify(image.pixel(probe_x, y)) == "red":
            top = y
            break
    if not record(
        "red square found at the top left",
        top is not None,
        "first red pixel in column %d at y=%s" % (probe_x, top),
    ):
        return results

    # 2. Its size gives the scale; it must be square.
    middle_y = top + 2
    left = probe_x - run_length(image, probe_x, middle_y, -1, 0, "red") + 1
    width = run_length(image, left, middle_y, 1, 0, "red")
    height = run_length(image, probe_x, top, 0, 1, "red")
    square = abs(width - height) <= max(2, width // 50)
    if not record(
        "red square is square and starts at the left edge",
        square and left <= max(2, image.width // 200),
        "%dx%d image pixels, left edge at x=%d" % (width, height, left),
    ):
        return results
    scale = width / float(SQUARE)
    content_width = image.width / scale
    content_height = (image.height - top) / scale
    if not record(
        "page is large enough to check",
        0.5 <= scale <= 4 and content_width >= 2 * SQUARE + 80 and content_height >= HEADING_BOTTOM + BAR + 60,
        "%.2f image pixels per CSS pixel; page %.0fx%.0f CSS pixels"
        % (scale, content_width, content_height),
    ):
        return results

    def at(css_x, css_y):
        x = int(round(css_x * scale))
        y = top + int(round(css_y * scale))
        return min(max(x, 0), image.width - 1), min(max(y, 0), image.height - 1)

    radius = max(1, int(scale))

    def expect(name, colour, points):
        bad = []
        for css_x, css_y in points:
            x, y = at(css_x, css_y)
            rgb = image.average(x, y, radius)
            if classify(rgb) != colour:
                bad.append("(%d,%d)=rgb%s" % (x, y, rgb))
        detail = "%d samples" % len(points) if not bad else "not %s at %s" % (colour, ", ".join(bad))
        record(name, not bad, detail)

    cw = content_width
    ch = content_height
    inset = 0.15 * SQUARE
    near = (inset, SQUARE / 2.0, SQUARE - inset)
    expect("red square", "red", [(x, y) for x in near for y in near])
    expect("blue square at the top right", "blue", [(cw - SQUARE + x, y) for x in near for y in near])
    bar_rows = (ch - BAR + 0.2 * BAR, ch - BAR / 2.0, ch - 0.2 * BAR)
    bar_columns = (0.03 * cw, 0.25 * cw, 0.5 * cw, 0.75 * cw, 0.97 * cw)
    expect("green bar along the bottom", "green", [(x, y) for x in bar_columns for y in bar_rows])

    gap = 24
    lower_middle = (HEADING_BOTTOM + ch - BAR) / 2.0
    expect(
        "white everywhere else",
        "white",
        [
            (cw / 2.0, SQUARE / 2.0),  # between the squares
            (SQUARE + gap, SQUARE / 2.0),  # just right of the red square
            (cw - SQUARE - gap, SQUARE / 2.0),  # just left of the blue square
            (SQUARE / 2.0, (SQUARE + HEADING_TOP) / 2.0),  # below the red square
            (cw - SQUARE / 2.0, (SQUARE + HEADING_TOP) / 2.0),  # below the blue square
            (0.05 * cw, lower_middle),  # left margin, below the heading
            (cw / 2.0, lower_middle),
            (0.95 * cw, lower_middle),
            (cw / 2.0, ch - BAR - gap),  # just above the green bar
        ],
    )
    return results


def main(argv):
    if len(argv) != 2 or argv[1] in ("-h", "--help"):
        sys.stderr.write("usage: %s <screenshot.png>\n" % argv[0])
        return 2
    path = argv[1]
    try:
        with open(path, "rb") as handle:
            data = handle.read(MAX_FILE_BYTES + 1)
        if len(data) > MAX_FILE_BYTES:
            raise PngError("file is larger than %d bytes" % MAX_FILE_BYTES)
        image = decode_png(data)
    except (OSError, PngError) as error:
        print("FAIL %s: not a usable PNG: %s" % (path, error))
        return 2
    except (struct.error, ValueError, IndexError, MemoryError) as error:
        # Every malformed input above has its own message; this keeps any
        # case they miss on the documented exit status rather than a traceback.
        print("FAIL %s: not a usable PNG: %s: %s" % (path, type(error).__name__, error))
        return 2
    print("image %s: %dx%d" % (path, image.width, image.height))
    results = check(image)
    for name, ok, detail in results:
        print("%s %s: %s" % ("PASS" if ok else "FAIL", name, detail))
    failed = [name for name, ok, _ in results if not ok]
    if failed:
        print("screenshot does not show tests/pages/colors.html as expected")
        return 1
    print("screenshot matches tests/pages/colors.html")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
