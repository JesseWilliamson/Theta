// Dev helper: full-page screenshots of every page at desktop and mobile sizes, in both themes.
// usage: NODE_PATH=$(npm root -g) node site/tools/shoot.mjs http://localhost:8731 /home/user/shots/site
import { createRequire } from "node:module";
const { chromium } = createRequire(import.meta.url)("playwright");
const [base, out] = process.argv.slice(2);
const pages = ["index", "keybindings", "changelog", "404"];
const browser = await chromium.launch();
for (const [w, h, tag] of [[1440, 900, "desktop"], [390, 844, "mobile"]]) {
  for (const scheme of ["dark", "light"]) {
    const ctx = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 1, colorScheme: scheme });
    const page = await ctx.newPage();
    for (const name of pages) {
      await page.goto(`${base}/${name}.html`, { waitUntil: "networkidle" });
      // lazy images only load when scrolled near; walk the page, then wait for every image to finish
      await page.evaluate(async () => {
        for (let y = 0; y < document.body.scrollHeight; y += 500) { window.scrollTo(0, y); await new Promise((r) => setTimeout(r, 60)); }
        window.scrollTo(0, 0);
        await Promise.all([...document.images].map((i) => (i.complete ? 0 : new Promise((r) => { i.onload = i.onerror = r; }))));
      });
      await page.waitForTimeout(300);
      await page.screenshot({ path: `${out}/${name}-${tag}-${scheme}.png`, fullPage: true });
    }
    await ctx.close();
  }
}
await browser.close();
