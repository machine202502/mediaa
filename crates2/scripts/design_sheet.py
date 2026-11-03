from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
SNAPSHOTS = ROOT / "crates" / "frontend" / "tests" / "snapshots"
OUT = ROOT / "target" / "design-contact-sheet.png"
PAGES = [
    ("library-populated", "Library"),
    ("library-empty", "Library · empty"),
    ("library-narrow", "Library · narrow"),
    ("library-bulk-selected", "Library · selected"),
    ("settings", "Settings · simple"),
    ("settings-advanced", "Settings · advanced"),
    ("capture", "Screenshot"),
    ("capture-narrow", "Screenshot · narrow"),
    ("record", "Record"),
    ("record-narrow", "Record · narrow"),
    ("record-recording", "Record · recording"),
    ("record-paused", "Record · paused"),
    ("record-saving", "Record · saving"),
    ("audio", "Audio"),
    ("audio-narrow", "Audio · narrow"),
    ("convert", "Convert"),
    ("convert-selected", "Convert · selected"),
    ("convert-running", "Convert · running"),
    ("convert-cancelling", "Convert · cancelling"),
]


def main() -> None:
    columns = 3
    width = 420
    rows = (len(PAGES) + columns - 1) // columns
    sheets = []
    font = ImageFont.load_default(size=16)
    for name, title in PAGES:
        path = SNAPSHOTS / f"{name}.png"
        if not path.is_file():
            continue
        image = Image.open(path).convert("RGB")
        height = round(image.height * width / image.width)
        image = image.resize((width, height), Image.Resampling.LANCZOS)
        header = Image.new("RGB", (width, 30), (18, 24, 39))
        draw = ImageDraw.Draw(header)
        draw.text((10, 7), title, fill=(235, 240, 250), font=font)
        sheet = Image.new("RGB", (width, height + 30), (18, 24, 39))
        sheet.paste(header, (0, 0))
        sheet.paste(image, (0, 30))
        sheets.append(sheet)
    cell_w = width
    cell_h = max(sheet.height for sheet in sheets) if sheets else 0
    canvas = Image.new(
        "RGB",
        (columns * cell_w, rows * cell_h),
        (10, 15, 26),
    )
    for index, sheet in enumerate(sheets):
        x = (index % columns) * cell_w
        y = (index // columns) * cell_h
        canvas.paste(sheet, (x, y))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(OUT)
    print(OUT)


if __name__ == "__main__":
    main()
