// node render.js <svg> <outdir> : rasterises the SVG with Chromium at every icon size.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('fs'), path = require('path');
(async () => {
  const [svgPath, out] = process.argv.slice(2);
  const svg = fs.readFileSync(svgPath, 'utf8');
  const b = await chromium.launch();
  for (const s of [16, 24, 32, 48, 64, 128, 256, 512, 1024]) {
    const p = await b.newPage({ viewport: { width: s, height: s } });
    await p.setContent(`<html><body style="margin:0;background:transparent"><div style="width:${s}px;height:${s}px">${svg.replace(/width="512" height="512"/, `width="${s}" height="${s}"`)}</div></body></html>`);
    await p.screenshot({ path: path.join(out, `quarry-${s}.png`), omitBackground: true });
    await p.close();
  }
  await b.close();
})();
