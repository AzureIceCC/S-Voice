"""Render UI mockup PNGs using Playwright.

This is a one-off dev helper — not part of the shipped app. It loads the
mockup HTML files from mockups/ and writes PNGs to mockups/.
"""

import asyncio
from pathlib import Path
from playwright.async_api import async_playwright

ROOT = Path(__file__).resolve().parent.parent
MOCKUPS = ROOT / "mockups"


async def render(viewport: dict, html_file: str, png_file: str, full_page: bool = False) -> None:
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        ctx = await browser.new_context(viewport=viewport, device_scale_factor=2)
        page = await ctx.new_page()
        await page.goto(f"file://{MOCKUPS / html_file}")
        # Let CSS / fonts settle
        await page.wait_for_load_state("networkidle")
        await page.wait_for_timeout(200)
        out = MOCKUPS / png_file
        await page.screenshot(path=str(out), full_page=full_page, omit_background=False)
        await browser.close()
        print(f"  wrote {out}  ({out.stat().st_size} bytes)")


async def main():
    print("settings window...")
    await render(
        viewport={"width": 480, "height": 540},
        html_file="settings-mock.html",
        png_file="settings.png",
    )
    print("floating panel (with desktop backdrop)...")
    await render(
        viewport={"width": 640, "height": 200},
        html_file="floating-mock.html",
        png_file="floating.png",
    )
    # Also render a tight crop of just the panel for clarity
    print("floating panel (tight crop)...")
    await render(
        viewport={"width": 320, "height": 120},
        html_file="floating-mock.html",
        png_file="floating-tight.png",
    )


if __name__ == "__main__":
    asyncio.run(main())
