// node render.js <svg> <outdir> [<small-svg>] : rasterises the SVG with Chromium at every icon size;
// sizes <= 24 px use <small-svg> (the heavier glyph that holds up at 16/24 px) when it is given.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('fs'), path = require('path');
(async () => {
  const [svgPath, out, smallPath] = process.argv.slice(2);
  const full = fs.readFileSync(svgPath, 'utf8');
  const small = smallPath ? fs.readFileSync(smallPath, 'utf8') : full;
  const b = await chromium.launch();
  for (const s of [16, 24, 32, 48, 64, 128, 256, 512, 1024]) {
    const svg = s <= 24 ? small : full;
    const p = await b.newPage({ viewport: { width: s, height: s } });
    await p.setContent(`<html><body style="margin:0;background:transparent"><div style="width:${s}px;height:${s}px">${svg.replace(/width="512" height="512"/, `width="${s}" height="${s}"`)}</div></body></html>`);
    await p.screenshot({ path: path.join(out, `quarry-${s}.png`), omitBackground: true });
    await p.close();
  }
  await b.close();
})();
