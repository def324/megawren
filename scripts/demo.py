#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["Pillow==13.0.0"]
# ///
"""Render the README tour from the current widgets and an isolated, synthetic drive."""
from collections import deque
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFont, ImageSequence

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"
# Background, foreground, then the normal and bright ANSI colors.
TOUR_THEME = (
    "090d12,f4f7fa,"
    "121820,e56d78,63d792,e6bd70,67a9fa,b795ed,60d9f7,e4eaf1,"
    "8090a0,ff8591,8be5a9,ffd68f,88bcff,ccaefc,87e6ff,ffffff"
)
MATTE = "#10171f"
TERMINAL_EDGE = "#475666"
CAPTION_FILL = "#1b2b36"
CAPTION_EDGE = "#547181"
CAPTION_INK = "#e2edf1"
INSET = 10


def tour_frame(terminal_size, captions, font):
    width, height = terminal_size
    width += 2 * INSET
    terminal_bottom = height + 2 * INSET
    center = (width // 2, terminal_bottom + 64)
    half = round(max(map(font.getlength, captions)) / 2) + 64
    if half > width // 2 - 24:
        raise ValueError("Shorten the caption to fit the tour")
    # Supersample the two outlines only. Paste the terminal and its glyphs at native resolution.
    size = (width, terminal_bottom + 128)
    surface = Image.new("RGB", (width * 2, size[1] * 2), MATTE)
    draw = ImageDraw.Draw(surface)
    draw.rounded_rectangle((2, 2, 2 * width - 3, 2 * terminal_bottom - 3),
                           radius=36, fill="#090d12", outline=TERMINAL_EDGE, width=6)
    x, y = center
    draw.rounded_rectangle((2 * (x - half), 2 * (y - 43), 2 * (x + half), 2 * (y + 43)),
                           radius=26, fill=CAPTION_FILL, outline=CAPTION_EDGE, width=4)
    return surface.resize(size, Image.Resampling.LANCZOS), center


def tour_palette(gif, surface, font, center, caption):
    # Sample the whole tour once so a moving progress bar can't recolor stationary text.
    width, height = gif.width // 6, gif.height // 6
    samples = Image.new("RGB", (width, height * (gif.n_frames + 1)), MATTE)
    still = surface.copy()
    ImageDraw.Draw(still).text(center, caption, font=font, anchor="mm", fill=CAPTION_INK)
    samples.paste(still.resize((width, height), Image.Resampling.NEAREST))
    for row, frame in enumerate(ImageSequence.Iterator(gif), start=1):
        sample = frame.convert("RGB").resize((width, height), Image.Resampling.NEAREST)
        samples.paste(sample, (0, height * row))
    gif.seek(0)
    return samples.quantize()


def add_captions(cast, terminal, output):
    """Typeset cast markers below the terminal, using the GIF's actual frame delays."""
    cues = deque()
    for line in cast.read_text().splitlines()[1:]:
        seconds, event, value = json.loads(line)
        if event == "m":
            cues.append((round(seconds * 1000), value))
    if not cues or cues[0][0] != 0:
        raise ValueError("The tour needs an opening caption")

    frames, delays = [], []
    font = ImageFont.load_default(size=38)
    now = 0
    with Image.open(terminal) as gif:
        canvas, center = tour_frame(gif.size, [caption for _, caption in cues], font)
        palette = tour_palette(gif, canvas, font, center, cues[0][1])
        caption_top = gif.height + 2 * INSET
        band = canvas.crop((0, caption_top, canvas.width, canvas.height))
        for frame in ImageSequence.Iterator(gif):
            canvas.paste(frame.convert("RGB"), (INSET, INSET))
            end = now + frame.info["duration"]
            while now < end:
                if cues and cues[0][0] <= now:
                    _, caption = cues.popleft()
                    canvas.paste(band, (0, caption_top))
                    ImageDraw.Draw(canvas).text(center, caption, font=font, anchor="mm", fill=CAPTION_INK)
                until = min(end, cues[0][0]) if cues else end
                frames.append(canvas.quantize(palette=palette, dither=Image.Dither.NONE))
                delays.append(until - now)
                now = until
    if cues:
        raise ValueError("A caption starts after the animation ends")
    frames[0].save(output, save_all=True, append_images=frames[1:], duration=delays,
                   loop=0, disposal=1, optimize=True)


def main():
    agg = shutil.which("agg") or shutil.which("agg", path=str(TARGET / "demo-tools"))
    if not agg:
        raise SystemExit("Install asciinema agg 1.9 or newer to render the GIF.")
    build = subprocess.run(
        [ROOT / "scripts/cargo.sh", "test", "--locked", "--offline", "--no-run",
         "--message-format=json", "--bin", "megawren"],
        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True,
    )
    executable, = (
        item["executable"] for item in map(json.loads, build.stdout.splitlines())
        if item["reason"] == "compiler-artifact" and item["target"]["name"] == "megawren"
        and item["profile"]["test"] and item["executable"]
    )
    TARGET.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=TARGET) as directory:
        work = Path(directory)
        cast = work / "tour.cast"
        # Only this test runs. Its home, working directory, temp files, and environment are private.
        subprocess.run(
            [executable, "--ignored", "--exact", "tui::preview::demo::record"], cwd=work,
            env={"HOME": directory, "TMPDIR": directory, "TZ": "UTC", "TERM": "xterm-256color",
                 "MEGAWREN_DEMO_CAST": str(cast)}, check=True,
        )
        terminal = work / "terminal.gif"
        # Use the same font metrics for rounded corners and adjoining lines, at 2× density.
        subprocess.run(
            [agg, "--renderer", "resvg", "--theme", TOUR_THEME, "--font-size", "32", "--line-height", "1.25",
             "--idle-time-limit", "86400", "--last-frame-duration", "4", cast, terminal], cwd=work, check=True,
        )
        gif = work / "megawren.gif"
        add_captions(cast, terminal, gif)
        output = ROOT / "docs/media" / gif.name
        os.replace(gif, output)
        print(output.relative_to(ROOT))


if __name__ == "__main__":
    main()
