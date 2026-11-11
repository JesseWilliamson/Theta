// Automated site check with Playwright: console errors, failed and external requests, link resolution,
// image alt + dimensions, horizontal overflow at 360px, tab order, and page weight / request counts.
// usage: NODE_PATH=$(npm root -g) node site/tools/check.mjs http://localhost:8731
import { createRequire } from "node:module";
const { chromium } = createRequire(import.meta.url)("playwright");
const base = process.argv[2].replace(/\/$/, "");
const origin = new URL(base).origin;
const pages = ["index.html", "keybindings.html", "changelog.html", "404.html"];
let problems = 0;
const bad = (m) => { problems++; console.log("  PROBLEM:", m); };

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });

for (const name of pages) {
  console.log(`\n== ${name}`);
  const page = await ctx.newPage();
  const reqs = [], failed = [], logs = [];
  let bytes = 0;
  page.on("console", (m) => { if (["error", "warning"].includes(m.type())) logs.push(m.text()); });
  page.on("pageerror", (e) => logs.push("pageerror: " + e.message));
  page.on("requestfailed", (r) => failed.push(r.url()));
  page.on("response", async (r) => {
    reqs.push({ url: r.url(), status: r.status() });
    try { bytes += (await r.body()).length; } catch { bytes += Number(r.headers()["content-length"] || 0); }
  });
  await page.goto(`${base}/${name}`, { waitUntil: "networkidle" });
  const initial = { n: reqs.length, bytes };
  // scroll to load lazy images, as a reader would
  await page.evaluate(async () => {
    for (let y = 0; y < document.body.scrollHeight; y += 400) { window.scrollTo(0, y); await new Promise((r) => setTimeout(r, 50)); }
    await Promise.all([...document.images].map((i) => (i.complete ? 0 : new Promise((r) => { i.onload = i.onerror = r; }))));
  });
  await page.waitForTimeout(500);
  if (process.env.LIST) reqs.forEach((r) => console.log("    ", r.status, r.url.replace(origin, "")));
  console.log(`  requests: ${initial.n} on load (${(initial.bytes / 1024).toFixed(0)} KB), ${reqs.length} after scrolling the whole page (${(bytes / 1024).toFixed(0)} KB)`);
  logs.forEach((l) => bad("console: " + l));
  failed.forEach((u) => bad("failed request: " + u));
  reqs.filter((r) => !r.url.startsWith(origin)).forEach((r) => bad("external request: " + r.url));
  reqs.filter((r) => r.status >= 400 && name !== "404.html").forEach((r) => bad(`HTTP ${r.status}: ${r.url}`));

  // images
  const imgs = await page.$$eval("img", (els) => els.map((i) => ({ src: i.currentSrc || i.src, alt: i.getAttribute("alt"), w: i.getAttribute("width"), h: i.getAttribute("height") })));
  imgs.forEach((i) => { if (!i.alt || !i.w || !i.h) bad(`img missing alt/width/height: ${i.src}`); });
  console.log(`  images: ${imgs.length}, all with alt + width/height: ${imgs.every((i) => i.alt && i.w && i.h)}`);

  // links
  const hrefs = await page.$$eval("a[href]", (as) => as.map((a) => a.getAttribute("href")));
  const internal = [...new Set(hrefs.filter((h) => !/^(https?:|mailto:)/.test(h)))];
  for (const h of internal) {
    const [file, frag] = h.split("#");
    const target = file || name;
    const res = await ctx.request.get(`${base}/${target}`);
    if (res.status() !== 200) { bad(`link ${h} -> ${res.status()}`); continue; }
    if (frag && target.endsWith(".html")) {
      const body = await res.text();
      if (!new RegExp(`id="${frag}"`).test(body)) bad(`link ${h}: no element with id "${frag}"`);
    }
  }
  console.log(`  internal links checked: ${internal.length}`);
  await page.close();
}
await ctx.close();

// 360px overflow and tab order
const mctx = await browser.newContext({ viewport: { width: 360, height: 740 }, deviceScaleFactor: 1 });
for (const name of pages) {
  const page = await mctx.newPage();
  await page.goto(`${base}/${name}`, { waitUntil: "networkidle" });
  await page.evaluate(async () => { for (let y = 0; y < document.body.scrollHeight; y += 500) { window.scrollTo(0, y); await new Promise((r) => setTimeout(r, 40)); } window.scrollTo(0, 0); });
  const over = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, iw: window.innerWidth }));
  console.log(`\n360px ${name}: scrollWidth ${over.sw} / viewport ${over.iw}`);
  if (over.sw > over.iw) {
    bad("horizontal overflow at 360px");
    const culprits = await page.evaluate(() => [...document.querySelectorAll("body *")].filter((e) => e.getBoundingClientRect().right > window.innerWidth + 1).slice(0, 6).map((e) => e.tagName + "." + e.className));
    console.log("  culprits:", culprits.join(", "));
  }
  await page.close();
}
await mctx.close();

const t = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const p = await t.newPage();
await p.goto(`${base}/index.html`, { waitUntil: "networkidle" });
const order = [];
for (let i = 0; i < 16; i++) {
  await p.keyboard.press("Tab");
  order.push(await p.evaluate(() => { const e = document.activeElement; return (e.tagName + " " + (e.getAttribute("aria-label") || e.textContent || e.getAttribute("href") || "").trim().replace(/\s+/g, " ")).slice(0, 40); }));
}
console.log("\ntab order (index, first 16):\n  " + order.join("\n  "));

// tabs keyboard operation
await p.locator("#tab-helix").focus();
await p.keyboard.press("ArrowRight");
const sel = await p.evaluate(() => [...document.querySelectorAll('#keymaps [role="tab"]')].map((t) => t.getAttribute("aria-selected")));
console.log("\nkeymap tabs after ArrowRight (aria-selected):", sel.join(","));
if (sel.join() !== "false,true,false") bad("ArrowRight did not move to the next tab");
await p.keyboard.press("End");
const sel2 = await p.evaluate(() => [...document.querySelectorAll('#keymaps [role="tab"]')].map((t) => t.getAttribute("aria-selected")));
if (sel2.join() !== "false,false,true") bad("End did not select the last tab");

// theme toggle persists across reloads
await p.evaluate(() => { try { localStorage.clear(); } catch {} });
await p.reload({ waitUntil: "networkidle" });
const before = await p.evaluate(() => document.documentElement.getAttribute("data-theme"));
await p.locator(".theme-toggle").click();
const after = await p.evaluate(() => document.documentElement.getAttribute("data-theme"));
await p.reload({ waitUntil: "networkidle" });
const kept = await p.evaluate(() => document.documentElement.getAttribute("data-theme"));
console.log(`theme toggle: ${before} -> ${after}, after reload: ${kept}`);
if (!after || after !== kept) bad("theme choice not persisted");

// reduced motion: the mock stays in its static state (popup open) and nothing animates
const rm = await browser.newContext({ reducedMotion: "reduce", viewport: { width: 1440, height: 900 } });
const rp = await rm.newPage();
await rp.goto(`${base}/index.html`, { waitUntil: "networkidle" });
await rp.locator("#joins").scrollIntoViewIfNeeded();
await rp.waitForTimeout(3500);
const popOpen = await rp.evaluate(() => { const e = document.querySelector("[data-pop]"); return !e.hidden && getComputedStyle(e).display !== "none"; });
console.log("reduced motion, popup still open after 3.5s:", popOpen);
if (!popOpen) bad("animation ran despite prefers-reduced-motion");
await rm.close();

// no-JS rendering sanity: every panel visible, no tablist
const nj = await browser.newContext({ javaScriptEnabled: false, viewport: { width: 1440, height: 900 } });
const np = await nj.newPage();
await np.goto(`${base}/index.html`, { waitUntil: "networkidle" });
const vis = await np.$$eval('#keymaps [role="tabpanel"]', (els) => els.map((e) => getComputedStyle(e).display !== "none"));
console.log("no-JS keymap panels visible:", vis.join(","));
if (!vis.every(Boolean)) bad("a keymap panel is hidden without JS");

console.log(`\n${problems === 0 ? "ALL CHECKS PASSED" : problems + " PROBLEM(S)"}`);
await browser.close();
process.exit(problems ? 1 : 0);
