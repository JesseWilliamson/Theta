// Renders the raster assets from their sources with Playwright + Chromium:
//   assets/og.png (1200x630 social card, from tools/og.html)
//   assets/favicon-32.png (from assets/favicon.svg) and assets/apple-touch-icon.png (from the full-detail app icon,
//   packaging/icons/theta.svg: at 180 px the delicate drawing resolves, the 32 px favicon drawing would be coarse)
// usage: node site/tools/render_assets.mjs   (needs `playwright` resolvable from the working directory)
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const here = path.dirname(fileURLToPath(import.meta.url));
const site = path.resolve(here, "..");

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
await page.goto(pathToFileURL(path.join(here, "og.html")).href);
await page.evaluate(() => document.fonts.ready);
await page.screenshot({ path: path.join(site, "assets/og.png") });

const sources = [
  [32, "favicon-32.png", path.join(site, "assets/favicon.svg")],
  [180, "apple-touch-icon.png", path.join(site, "../packaging/icons/theta.svg")],
];
for (const [size, name, file] of sources) {
  const p = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
  // Inline the SVG with its fixed 512 px width/height replaced, so it scales to the box (same trick as
  // packaging/icons/render.js).
  const svg = fs.readFileSync(file, "utf8").replace(/width="512" height="512"/, `width="${size}" height="${size}"`);
  await p.setContent(`<body style="margin:0;background:transparent">${svg}`);
  await p.screenshot({ path: path.join(site, "assets", name), omitBackground: true });
}
await browser.close();
console.log("rendered og.png, favicon-32.png, apple-touch-icon.png");
