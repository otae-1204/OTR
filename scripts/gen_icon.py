"""生成 Token Show 应用图标源图(512x512 PNG),无需第三方依赖"""
import struct
import zlib

W = H = 512
R = 96  # 圆角半径

# 渐变端色:#0EA5E9 -> #2563EB
TOP = (14, 165, 233)
BOT = (37, 99, 235)


def lerp(a, b, t):
    return int(a + (b - a) * t)


def in_rounded(x, y):
    """左上原点坐标系,y 向下"""
    cx = min(max(x, R), W - 1 - R)
    cy = min(max(y, R), H - 1 - R)
    return (x - cx) ** 2 + (y - cy) ** 2 <= R * R


def in_bar(x, y):
    return 136 <= x < 376 and 136 <= y < 196


def in_stem(x, y):
    return 224 <= x < 288 and 196 <= y < 400


def pixel(x, y):
    if not in_rounded(x, y):
        return (0, 0, 0, 0)
    t = y / (H - 1)
    r, g, b = lerp(TOP[0], BOT[0], t), lerp(TOP[1], BOT[1], t), lerp(TOP[2], BOT[2], t)
    if in_bar(x, y) or in_stem(x, y):
        return (255, 255, 255, 255)
    return (r, g, b, 255)


def build(path):
    raw = bytearray()
    for y in range(H):
        raw.append(0)  # filter type 0
        for x in range(W):
            raw.extend(pixel(x, y))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)
    print("written:", path)


if __name__ == "__main__":
    build("scripts/icon-source.png")
