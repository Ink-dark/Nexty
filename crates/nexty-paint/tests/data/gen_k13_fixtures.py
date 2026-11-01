#!/usr/bin/env python3
"""K13 解码 fixture 生成（幂等）：python gen_k13_fixtures.py 重新生成全部 fixture。

测试图与超限头部 fixture 入库 `tests/data/`，Rust 侧测试读文件不现场编码；
本脚本保留可读源，Pillow 版本变更后可重跑复核。颜色与几何固定，输出确定性。

fixture 一览（命名 k13-*）：
- k13-8x8.{png,jpeg,gif,webp}  8x8 四象限色块（左上红/右上绿/左下蓝/右下白），
  jpeg/webp 有损（quality=95），像素断言允许通道偏差；
- k13-8x8-anim.{gif,webp}      两帧动图（帧 1 全红、帧 2 全蓝），解码应取帧 1；
- k13-exif-rot.jpeg            左半红右半蓝 8x4 + EXIF Orientation=6（转 90°），
  解码不做方向变换，尺寸应保持 8x4；
- k13-oversize-20000x1.jpeg    边长超 MAX_DIMENSION（16384）的纯色图；
- k13-oversize-16384x5000.jpeg 边长在限内、面积超 MAX_PIXEL_AREA（2^26）的纯色图；
- k13-16x16.ico                16x16 橙色 ICO（favicon 解码路径 + 验收站复用）。
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent

RED = (255, 0, 0, 255)
GREEN = (0, 255, 0, 255)
BLUE = (0, 0, 255, 255)
WHITE = (255, 255, 255, 255)
ORANGE = (255, 136, 0, 255)


def quadrant_image(size=8):
    """四象限色块：左上红 / 右上绿 / 左下蓝 / 右下白（象限中心可精确采样）。"""
    image = Image.new("RGBA", (size, size), WHITE)
    half = size // 2
    for y in range(half):
        for x in range(half):
            image.putpixel((x, y), RED)
            image.putpixel((x + half, y), GREEN)
            image.putpixel((x, y + half), BLUE)
    return image


def solid(size, color):
    return Image.new("RGBA", size, color)


def save(image, name, **kwargs):
    path = HERE / name
    image.save(path, **kwargs)
    print(f"{name}: {path.stat().st_size} bytes")


def main():
    quadrant = quadrant_image()
    save(quadrant, "k13-8x8.png")
    # JPEG 无 alpha：转 RGB（颜色不变，jpeg 有损，像素断言允许偏差）。
    # subsampling=0（4:4:4）：8x8 恰好只有一个 DCT 块，4:2:0 会把整幅图
    # 的色度混在一起，象限色不可辨认
    save(quadrant.convert("RGB"), "k13-8x8.jpeg", quality=95, subsampling=0)
    # GIF 无 alpha：转 RGB 后按调色板存储，四种纯色精确存活
    save(quadrant.convert("RGB"), "k13-8x8.gif")
    save(quadrant, "k13-8x8.webp", quality=95)

    # 动图：帧 1 全红、帧 2 全蓝——解码取帧 1（红）
    save(
        solid((8, 8), RED),
        "k13-8x8-anim.gif",
        save_all=True,
        append_images=[solid((8, 8), BLUE)],
        duration=100,
        loop=0,
    )
    save(
        solid((8, 8), RED),
        "k13-8x8-anim.webp",
        save_all=True,
        append_images=[solid((8, 8), BLUE)],
        duration=100,
        loop=0,
    )

    # EXIF Orientation=6（顺时针转 90°）：左半红右半蓝 8x4；解码不应用方向
    exif_image = Image.new("RGBA", (8, 4), BLUE)
    for y in range(4):
        for x in range(4):
            exif_image.putpixel((x, y), RED)
    exif = exif_image.getexif()
    exif[274] = 6  # 274 = Orientation tag
    save(
        exif_image.convert("RGB"),
        "k13-exif-rot.jpeg",
        quality=95,
        subsampling=0,
        exif=exif.tobytes(),
    )

    # 超限预检 fixture：纯色 jpeg，头部声明巨尺寸，预检在像素解码前拒绝
    #（永不进入像素解码，quality 只影响入库体积）
    save(solid((20000, 1), RED).convert("RGB"), "k13-oversize-20000x1.jpeg", quality=40, optimize=True)
    save(solid((16384, 5000), GREEN).convert("RGB"), "k13-oversize-16384x5000.jpeg", quality=40, optimize=True)

    # ICO（favicon）：16x16 纯橙，验收站 /favicon.ico 路由复用同一份
    save(solid((16, 16), ORANGE), "k13-16x16.ico")


if __name__ == "__main__":
    main()
