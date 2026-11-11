// Dev helper: focused crops (hero, editor mock states, keymap tabs, mobile nav, key binding search).
// usage: NODE_PATH=$(npm root -g) node site/tools/shoot_focus.mjs http://localhost:8731 /home/user/shots/site
import { createRequire } from "node:module";
const { chromium } = createRequire(import.meta.url)("playwright");
const [base, out] = process.argv.slice(2);
const browser = await chromium.launch();

async function open(ctx, name) {
  const page = await ctx.newPage();
  await page.goto(`${base}/${name}.html`, { waitUntil: "networkidle" });
  return page;
}

for (const scheme of ["dark", "light"]) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, colorScheme: scheme });
  const page = await open(ctx, "index");
  await page.screenshot({ path: `${out}/focus-hero-${scheme}.png` });

  // editor mock: static state, mid-animation, final state
  const mock = page.locator("#joins");
  await mock.scrollIntoViewIfNeeded();
  await page.waitForTimeout(800);
  await mock.screenshot({ path: `${out}/focus-mock-typing-${scheme}.png` });
  await page.waitForTimeout(1500);
  await mock.screenshot({ path: `${out}/focus-mock-popup-${scheme}.png` });
  await page.waitForTimeout(2000);
  await mock.screenshot({ path: `${out}/focus-mock-accepted-${scheme}.png` });

  // keymap tabs
  const km = page.locator("#keymaps .tabs");
  for (const tab of ["helix", "vim", "standard"]) {
    await page.locator(`#tab-${tab}`).click();
    await page.waitForTimeout(100);
    await km.screenshot({ path: `${out}/focus-tabs-${tab}-${scheme}.png` });
  }
  await ctx.close();
}

// mobile nav
{
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, colorScheme: "dark" });
  const page = await open(ctx, "index");
  await page.screenshot({ path: `${out}/focus-nav-mobile-closed.png`, clip: { x: 0, y: 0, width: 390, height: 120 } });
  await page.locator(".menu-btn").click();
  await page.screenshot({ path: `${out}/focus-nav-mobile-open.png`, clip: { x: 0, y: 0, width: 390, height: 420 } });
  await ctx.close();
}

// key bindings: search and tabs
{
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1, colorScheme: "dark" });
  const page = await open(ctx, "keybindings");
  await page.screenshot({ path: `${out}/focus-keys-helix.png` });
  await page.locator("#tab-vim").click();
  await page.screenshot({ path: `${out}/focus-keys-vim.png` });
  await page.locator("#q").fill("macro");
  await page.waitForTimeout(150);
  await page.screenshot({ path: `${out}/focus-keys-search.png` });
  await ctx.close();
}
await browser.close();
