// Renders the raster assets from their sources with Playwright + Chromium:
//   assets/og.png (1200x630 social card, from tools/og.html)
//   assets/favicon-32.png and assets/apple-touch-icon.png (from assets/favicon.svg)
// usage: node site/tools/render_assets.mjs   (needs `playwright` resolvable from the working directory)
import { createRequire } from "node:module";
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

const svg = pathToFileURL(path.join(site, "assets/favicon.svg")).href;
for (const [size, name] of [[32, "favicon-32.png"], [180, "apple-touch-icon.png"]]) {
  const p = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
  await p.goto(svg); // a bare SVG with a viewBox scales to fill the viewport
  await p.screenshot({ path: path.join(site, "assets", name), omitBackground: true });
}
await browser.close();
console.log("rendered og.png, favicon-32.png, apple-touch-icon.png");
