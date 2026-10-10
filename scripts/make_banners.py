#!/usr/bin/env python3
"""Render the ChannelFlow plugin repository banners.

Each plugin gets a wide banner (1200x400) in the style Jellyfin's plugin
catalog shows: a brand gradient, the plugin name, a one-line description, and
a small identity chip. Banners land in `banners/<plugin-id>.png` and are
referenced from manifest.json's `imageUrl`. Deterministic — rerunning it
produces byte-identical files.
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "banners"

WIDTH, HEIGHT = 1200, 400
CORNER = 28
MARK_SIZE = 100
MARGIN = 64

FONTS = ROOT / "scripts" / "fonts"  # fallback path if present
SYSTEM_FONTS = Path("/usr/share/fonts/rsms-inter-fonts")


def font(weight: str, size: int) -> ImageFont.FreeTypeFont:
    for base in (ROOT / "fonts", SYSTEM_FONTS):
        candidate = base / f"InterDisplay-{weight}.ttf"
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size)
    return ImageFont.load_default()


def interpolate(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))


def gradient(size: tuple[int, int], top, bottom) -> Image.Image:
    w, h = size
    image = Image.new("RGB", size)
    draw = ImageDraw.Draw(image)
    for y in range(h):
        color = interpolate(top, bottom, y / (h - 1))
        draw.line([(0, y), (w, y)], fill=color)
    return image


def waveform(draw: ImageDraw.ImageDraw, left: int, right: int, bottom: int, alpha: int = 26) -> None:
    """A row of rounded bars, the "channel flow" heartbeat, in translucent white."""
    heights = [36, 64, 46, 88, 58, 42, 76, 52, 96, 70, 44, 60, 82, 50, 38]
    slot = (right - left) / (len(heights) * 2)
    bar = max(14, slot * 0.55)
    for i, h in enumerate(heights):
        x = left + slot * (2 * i + 0.5)
        y0 = bottom - max(24, h)
        draw.rounded_rectangle(
            [(x, y0), (x + bar, bottom)],
            radius=bar / 2,
            fill=(255, 255, 255, alpha),
        )


def rounded_mask(size: tuple[int, int], radius: int) -> Image.Image:
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([(0, 0), (size[0] - 1, size[1] - 1)], radius=radius, fill=255)
    return mask


def render(plugin: dict) -> Image.Image:
    image = gradient((WIDTH, HEIGHT), plugin["top"], plugin["bottom"]).convert("RGBA")

    # A faint light pool in the top-right.
    overlay = Image.new("RGBA", (WIDTH, HEIGHT), (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay)
    draw.ellipse(
        [(WIDTH - 540, -240), (WIDTH + 20, 320)],
        fill=(255, 255, 255, 14),
    )

    # The heartbeat waveform, right of center.
    waveform(draw, 780, WIDTH - MARGIN, HEIGHT - 26, alpha=22)

    # App badge: white rounded square with the play mark, plus a soft rim.
    bx, by = MARGIN, (HEIGHT - MARK_SIZE) // 2
    draw.rounded_rectangle(
        [(bx, by), (bx + MARK_SIZE, by + MARK_SIZE)],
        radius=28,
        fill=(255, 255, 255, 255),
    )
    cx, cy, r = bx + MARK_SIZE // 2, by + MARK_SIZE // 2, 20
    triangle = [
        (cx + r * 0.42, cy - r * 0.95),
        (cx + r * 0.42, cy + r * 0.95),
        (cx + r * 1.05, cy),
    ]
    draw.polygon(triangle, fill=plugin["mark"])

    title_font = font("Bold", 72)
    subtitle_font = font("Regular", 27)
    chip_font = font("SemiBold", 25)
    small_font = font("Medium", 24)

    tx = bx + MARK_SIZE + 44
    max_text = WIDTH - MARGIN - tx
    title = plugin["title"]
    while title_font.getlength(title) > max_text and title_font.size > 36:
        title_font = font("Bold", title_font.size - 2)
    subtitle = plugin["subtitle"]
    while subtitle_font.getlength(subtitle) > max_text and subtitle_font.size > 20:
        subtitle_font = font("Regular", subtitle_font.size - 1)

    ty = by + 62
    draw.text((tx, ty), title, font=title_font, fill=(255, 255, 255, 255), anchor="ls")
    draw.text((tx, ty + 56), subtitle, font=subtitle_font, fill=(255, 255, 255, 235), anchor="ls")

    # Identity chips: plugin id and the base this targets.
    def chip(text: str, font: ImageFont.FreeTypeFont, y: int, x: int) -> tuple[int, int]:
        pad_x, pad_y = 18, 9
        w = round(font.getlength(text)) + pad_x * 2
        h = font.size + pad_y * 2
        draw.rounded_rectangle([(x, y), (x + w, y + h)], radius=h // 2, fill=(255, 255, 255, 30))
        draw.rounded_rectangle(
            [(x, y), (x + w, y + h)],
            radius=h // 2,
            outline=(255, 255, 255, 90),
            width=1,
        )
        draw.text((x + pad_x, y + h // 2), text, font=font, fill=(255, 255, 255, 255), anchor="ls")
        return x + w, y + h

    _cx, _cy = chip(plugin["id"], chip_font, ty + 92, tx)
    _cx, _cy = chip("ChannelFlow 2.0.0", small_font, ty + 92 + 2, _cx + 14)

    image.alpha_composite(overlay)

    # Outer rounded corners and a hairline rim.
    image.putalpha(rounded_mask((WIDTH, HEIGHT), CORNER))
    rim = Image.new("RGBA", (WIDTH, HEIGHT), (0, 0, 0, 0))
    ImageDraw.Draw(rim).rounded_rectangle(
        [(0, 0), (WIDTH - 1, HEIGHT - 1)],
        radius=CORNER,
        outline=(255, 255, 255, 60),
        width=2,
    )
    image.alpha_composite(rim)
    return image


PLUGINS = [
    {
        "id": "com.channelflow.ai",
        "title": "AI Provider Suite",
        "subtitle": "OpenAI-compatible endpoints, tests, and priority-ordered failover",
        "top": (67, 56, 202),   # indigo-700
        "bottom": (14, 165, 233),  # sky-500
        "mark": (30, 27, 75),   # indigo-950
    },
    {
        "id": "com.channelflow.ersatztv",
        "title": "ErsatzTV Transcoding Engine",
        "subtitle": "ffmpeg and normalization settings, mirrored from ErsatzTV next",
        "top": (15, 118, 110),   # teal-700
        "bottom": (6, 182, 212),  # cyan-500
        "mark": (4, 47, 46),     # teal-950
    },
    {
        "id": "com.channelflow.jellyfin",
        "title": "Jellyfin Media Source",
        "subtitle": "Sync movies, TV and music from a Jellyfin server",
        "top": (160, 55, 32),    # jellyfin's burnt sienna
        "bottom": (212, 120, 60),  # amber-600
        "mark": (64, 20, 10),    # deep brown
    },
    {
        "id": "com.channelflow.offair",
        "title": "Off Air",
        "subtitle": "What plays when a channel has no scheduled media",
        "top": (55, 61, 65),     # slate gray
        "bottom": (30, 41, 59),  # deep navy
        "mark": (235, 235, 235), # near-white play mark
    },
    {
        "id": "com.channelflow.commercialbrainz",
        "title": "CommercialBrainz",
        "subtitle": "Ad avails synced from CommercialBrainz",
        "top": (146, 64, 14),    # burnt orange
        "bottom": (217, 119, 6), # amber-600
        "mark": (46, 16, 4),     # deep brown
    },
    {
        "id": "com.channelflow.emergency",
        "title": "Emergency Broadcast System",
        "subtitle": "Weather alerts that overlay programming",
        "top": (153, 27, 27),    # alert red
        "bottom": (249, 115, 22),# orange-500
        "mark": (255, 255, 255), # white play mark
    },
    {
        "id": "com.channelflow.weather",
        "title": "Weather",
        "subtitle": "WeatherStar look, source, location, and screens",
        "top": (2, 132, 199),    # sky blue
        "bottom": (128, 222, 234), # cyan-300
        "mark": (12, 74, 110),   # deep blue
    },
    {
        "id": "com.channelflow.news",
        "title": "News",
        "subtitle": "The FlowWire newscast — headlines, read with TTS",
        "top": (30, 58, 138),    # news navy
        "bottom": (37, 99, 235), # blue-600
        "mark": (255, 255, 255), # white play mark
    },
]


def main() -> int:
    OUT.mkdir(exist_ok=True)
    for plugin in PLUGINS:
        target = OUT / f"{plugin['id']}.png"
        render(plugin).save(target)
        print(f"wrote {target.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())