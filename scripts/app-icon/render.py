"""Renders the app icon from crates/klipolo-app/icons/icon.svg, into the files tauri.conf.json bundles.

- icon.png (512), 32x32.png, 128x128.png and 128x128@2x.png (256).
- icon.icns, with every size macOS asks for, 16 to 1024, each stored as a PNG.

After the drawing changes: edit icon.svg and run render.sh.
"""
import pathlib
import struct

from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[1]
ICONS = REPO / "crates/klipolo-app/icons"
PNGS = {"icon.png": 512, "32x32.png": 32, "128x128.png": 128, "128x128@2x.png": 256}
# The icns entry types that hold a PNG, with their sizes in pixels.
ICNS = (("icp4", 16), ("icp5", 32), ("ic11", 32), ("ic12", 64), ("ic07", 128), ("ic13", 256), ("ic08", 256),
        ("ic14", 512), ("ic09", 512), ("ic10", 1024))


def icns(entries):
    body = b"".join(kind.encode() + struct.pack(">I", len(png) + 8) + png for kind, png in entries)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def main():
    source = (ICONS / "icon.svg").as_uri()
    sizes = sorted({s for s in PNGS.values()} | {s for _, s in ICNS})
    rendered = {}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(device_scale_factor=1)
        for size in sizes:
            page.set_viewport_size({"width": size, "height": size})
            page.goto(source)
            rendered[size] = page.screenshot(omit_background=True)
        browser.close()
    for name, size in PNGS.items():
        (ICONS / name).write_bytes(rendered[size])
        print(f"{name}: {size}x{size}")
    (ICONS / "icon.icns").write_bytes(icns([(kind, rendered[size]) for kind, size in ICNS]))
    print(f"icon.icns: {len(ICNS)} sizes, {(ICONS / 'icon.icns').stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
