"""Renders the site's brand images, then stamps the brand version into every link to them.

- site/assets/og.png: the share image, 1200x630, from og.html.
- site/assets/icon-192.png and icon-512.png: the web manifest's icons, from site/assets/klipolo-icon.svg.
- site/assets/favicon.ico (16, 32 and 48) and apple-touch-icon.png (180, square), from site/assets/favicon.svg.

The version in VERSION goes into each link as ?v=<version> (the server caches /assets/ for 30 days), in
site/*.html and site/site.webmanifest. After a new logo lands: replace klipolo-icon.svg and favicon.svg, bump
VERSION, and run render.sh.
"""
import functools
import http.server
import pathlib
import re
import struct
import threading

from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parents[1]
SITE = REPO / "site"
OG_MAX_BYTES = 300 * 1024
# The favicon's tile colour, behind the square touch icon so iOS's own rounding leaves no corners.
TILE = "#1a1c20"
BRAND_FILES = ("favicon.ico", "favicon.svg", "apple-touch-icon.png", "klipolo-icon.svg", "og.png", "icon-192.png", "icon-512.png")


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve():
    handler = functools.partial(Quiet, directory=str(REPO))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}"


def render(base):
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1200, "height": 630}, device_scale_factor=1)
        page.goto(f"{base}/scripts/site-brand/og.html", wait_until="networkidle")
        page.evaluate("document.fonts.ready")
        page.screenshot(path=str(SITE / "assets/og.png"))
        for size in (192, 512):
            page.set_viewport_size({"width": size, "height": size})
            page.goto(f"{base}/site/assets/klipolo-icon.svg", wait_until="networkidle")
            page.screenshot(path=str(SITE / f"assets/icon-{size}.png"), omit_background=True)
        icons = []
        for size in (16, 32, 48):
            page.set_viewport_size({"width": size, "height": size})
            page.goto(f"{base}/site/assets/favicon.svg", wait_until="networkidle")
            icons.append((size, page.screenshot(omit_background=True)))
        (SITE / "assets/favicon.ico").write_bytes(ico(icons))
        page.set_viewport_size({"width": 180, "height": 180})
        page.goto("about:blank")
        page.set_content(
            f'<body style="margin:0;background:{TILE}"><img src="{base}/site/assets/favicon.svg" width="180" height="180"></body>'
        )
        page.wait_for_load_state("networkidle")
        page.screenshot(path=str(SITE / "assets/apple-touch-icon.png"))
        browser.close()


def ico(pngs):
    """A .ico file that holds each (size, PNG) as is, which every current browser reads."""
    head = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries, data = b"", b""
    for size, png in pngs:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    return head + entries + data


def stamp(version):
    pattern = re.compile(r"(?<=[/\"'])(" + "|".join(re.escape(f) for f in BRAND_FILES) + r")(\?v=[\w.-]+)?")
    files = sorted(SITE.glob("*.html")) + [SITE / "site.webmanifest"]
    for path in files:
        if not path.exists():
            continue
        text = path.read_text()
        new = pattern.sub(lambda m: f"{m.group(1)}?v={version}", text)
        if new != text:
            path.write_text(new)
            print(f"stamped v={version}: {path.relative_to(REPO)}")


def main():
    version = (HERE / "VERSION").read_text().strip()
    server, base = serve()
    try:
        render(base)
    finally:
        server.shutdown()
    size = (SITE / "assets/og.png").stat().st_size
    print(f"site/assets/og.png: {size // 1024} KB")
    if size > OG_MAX_BYTES:
        raise SystemExit(f"og.png is over {OG_MAX_BYTES // 1024} KB: simplify og.html")
    stamp(version)


if __name__ == "__main__":
    main()
