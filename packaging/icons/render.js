// node render.js <svg> <outdir> <mid-svg> <small-svg> : rasterises the SVG with Chromium at every icon size.
// Three detail levels (see gen_icon.py): >= 64 px uses <svg>, 32 and 48 px use <mid-svg>, <= 24 px uses
// <small-svg>. Each has a stroke weight chosen for the pixel grid it is drawn on.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('fs'), path = require('path');
(async () => {
  const [svgPath, out, midPath, smallPath] = process.argv.slice(2);
  const full = fs.readFileSync(svgPath, 'utf8');
  const mid = midPath ? fs.readFileSync(midPath, 'utf8') : full;
  const small = smallPath ? fs.readFileSync(smallPath, 'utf8') : full;
  const b = await chromium.launch();
  for (const s of [16, 24, 32, 48, 64, 128, 256, 512, 1024]) {
    const svg = s <= 24 ? small : s <= 48 ? mid : full;
    const p = await b.newPage({ viewport: { width: s, height: s } });
    await p.setContent(`<html><body style="margin:0;background:transparent"><div style="width:${s}px;height:${s}px">${svg.replace(/width="512" height="512"/, `width="${s}" height="${s}"`)}</div></body></html>`);
    await p.screenshot({ path: path.join(out, `theta-${s}.png`), omitBackground: true });
    await p.close();
  }
  await b.close();
})();
