// One-off helper: cut the 11 round fruits out of the Q版水果 sprite sheet (checkerboard background, no alpha)
// into square source PNGs consumed by prepare-skins.mjs.
import sharp from "sharp";
import path from "node:path";
import {fileURLToPath} from "node:url";

const folder = fileURLToPath(new URL("../plugin/assets/skins/qfruit/source/", import.meta.url));
const {data, info} = await sharp(path.join(folder, "sheet.png")).raw().toBuffer({resolveWithObject: true});
const {width, height, channels} = info;
const fg = new Uint8Array(width * height);
for (let i = 0; i < width * height; i++) {
  const r = data[i*channels], g = data[i*channels+1], b = data[i*channels+2];
  const max = Math.max(r,g,b), min = Math.min(r,g,b);
  fg[i] = (max - min > 40 || max < 150) ? 1 : 0;
}
// Connected components (4-neighbour), keep large blobs.
const label = new Int32Array(width * height);
const blobs = [];
for (let start = 0; start < width * height; start++) {
  if (!fg[start] || label[start]) continue;
  const id = blobs.length + 1, stack = [start];
  label[start] = id;
  let x0 = width, y0 = height, x1 = 0, y1 = 0, count = 0;
  while (stack.length) {
    const p = stack.pop(), x = p % width, y = (p - x) / width;
    count++; if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y;
    for (const q of [x > 0 ? p-1 : -1, x < width-1 ? p+1 : -1, y > 0 ? p-width : -1, y < height-1 ? p+width : -1])
      if (q >= 0 && fg[q] && !label[q]) { label[q] = id; stack.push(q); }
  }
  blobs.push({x0, y0, x1, y1, count});
}
const fruits = blobs.filter(b => b.x1 - b.x0 > 150 && b.y1 - b.y0 > 150)
  .sort((a, b) => (Math.round(a.y0 / 200) - Math.round(b.y0 / 200)) || a.x0 - b.x0);
if (fruits.length !== 11) throw new Error(`Expected 11 fruits, found ${fruits.length}: ${JSON.stringify(fruits)}`);
for (const [i, b] of fruits.entries()) {
  const cx = (b.x0 + b.x1 + 1) / 2, cy = (b.y0 + b.y1 + 1) / 2;
  const side = Math.round(Math.max(b.x1 - b.x0 + 1, b.y1 - b.y0 + 1));
  const left = Math.round(cx - side / 2), top = Math.round(cy - side / 2);
  // Pad with white if the square slightly exceeds the sheet; the rim is cut round later anyway.
  const padded = await sharp(path.join(folder, "sheet.png"))
    .extend({top: 20, bottom: 20, left: 20, right: 20, background: "#ffffff"}).png().toBuffer();
  await sharp(padded)
    .extract({left: left + 20, top: top + 20, width: side, height: side})
    .png().toFile(path.join(folder, `fruit-${i+1}.png`));
  console.log(`fruit-${i+1}`, {left, top, side});
}
