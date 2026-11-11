// node render.js <svg> <outdir> <mid-svg> <small-svg> <svg-24> : rasterises the SVG with Chromium at every icon size.
// Four drawings (see gen_icon.py): >= 64 px uses <svg>, 32 and 48 px use <mid-svg>, 24 px uses <svg-24> and
// 16 px uses <small-svg>. Each is drawn on the pixel grid of the size it is used at.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('fs'), path = require('path');
(async () => {
  const [svgPath, out, midPath, smallPath, path24] = process.argv.slice(2);
  const full = fs.readFileSync(svgPath, 'utf8');
  const mid = midPath ? fs.readFileSync(midPath, 'utf8') : full;
  const small = smallPath ? fs.readFileSync(smallPath, 'utf8') : full;
  const small24 = path24 ? fs.readFileSync(path24, 'utf8') : small;
  const b = await chromium.launch();
  for (const s of [16, 24, 32, 48, 64, 128, 256, 512, 1024]) {
    const svg = s <= 16 ? small : s <= 24 ? small24 : s <= 48 ? mid : full;
    const p = await b.newPage({ viewport: { width: s, height: s } });
    await p.setContent(`<html><body style="margin:0;background:transparent"><div style="width:${s}px;height:${s}px">${svg.replace(/width="512" height="512"/, `width="${s}" height="${s}"`)}</div></body></html>`);
    await p.screenshot({ path: path.join(out, `theta-${s}.png`), omitBackground: true });
    await p.close();
  }
  await b.close();
})();
