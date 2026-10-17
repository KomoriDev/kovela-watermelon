import sharp from "sharp";
import {mkdir, readFile, writeFile} from "node:fs/promises";
import path from "node:path";
import {fileURLToPath} from "node:url";

const root = fileURLToPath(new URL("../plugin/assets/skins/", import.meta.url));
const sizes = [36,46,58,72,89,108,128,149,173,192,192];
const names = ["葡萄","樱桃","橙子","柠檬","猕猴桃","番茄","桃子","菠萝","椰子","西瓜","大西瓜"];
const sets = [{id:"classic",name:"经典水果"},{id:"qfruit",name:"Q版水果"}];
// Anti-aliased circular cut (4x4 supersampling). Source artwork is cropped tightly against the canvas, so a hard
// cut leaves flattened sides, jagged steps and stray corner pixels; keep the smaller of source alpha and coverage.
function roundCut(rgba,size) {
  const radius=size/2,samples=4;
  for(let y=0;y<size;y++)for(let x=0;x<size;x++) {
    let inside=0;
    for(let sy=0;sy<samples;sy++)for(let sx=0;sx<samples;sx++) {const dx=x+(sx+.5)/samples-radius,dy=y+(sy+.5)/samples-radius;if(dx*dx+dy*dy<=radius*radius)inside++;}
    const p=(y*size+x)*4;let alpha=Math.min(rgba[p+3],Math.round(255*inside/(samples*samples)));
    if(alpha<8)alpha=0;else if(alpha>247)alpha=255;
    rgba[p+3]=alpha;if(alpha===0)rgba[p]=rgba[p+1]=rgba[p+2]=0;
  }
}
// LVGL v9 I8: index 0 is fully transparent, indices 1..255 come from libimagequant (via sharp), including
// semi-transparent edge colours, so the anti-aliased rim survives quantization. Deterministic for fixed inputs.
async function indexed(rgba,width) {
  const quantized=await sharp(await sharp(rgba,{raw:{width,height:width,channels:4}}).png({palette:true,colours:255,dither:0,effort:10}).toBuffer()).ensureAlpha().raw().toBuffer();
  const bytes=Buffer.alloc(1036+width*width),lookup=new Map();
  bytes.set([0x19,0x0a,0,0]);bytes.writeUInt16LE(width,4);bytes.writeUInt16LE(width,6);bytes.writeUInt16LE(width,8);
  for(let i=0;i<width*width;i++) {
    const p=i*4;if(quantized[p+3]===0)continue;
    const key=quantized.readUInt32LE(p);let index=lookup.get(key);
    if(index===undefined) {index=lookup.size+1;if(index>255)throw new Error("Palette overflow");lookup.set(key,index);bytes.set([quantized[p+2],quantized[p+1],quantized[p],quantized[p+3]],12+index*4);}
    bytes[1036+i]=index;
  }
  return bytes;
}
// Preview PNG shows exactly what the device renders from the binary.
function decoded(bin,width) {
  const rgba=Buffer.alloc(width*width*4);
  for(let i=0;i<width*width;i++) {const o=12+bin[1036+i]*4;rgba.set([bin[o+2],bin[o+1],bin[o],bin[o+3]],i*4);}
  return rgba;
}
for(const set of sets) {
  const folder=path.join(root,set.id);await mkdir(path.join(folder,"source"),{recursive:true});
  for(let level=0;level<11;level++) {
    const size=sizes[level],stem=`fruit-${level+1}`;
    const source=await readFile(path.join(folder,"source",stem+".png"));let pipeline;
    if(set.id==="classic") pipeline=sharp(source).rotate().resize(size,size,{fit:"cover"});
    else {
      // Q版水果 sources are cut from a checkerboard sprite sheet (scripts/crop-qfruit.mjs); trim the anti-aliased rim
      // so no background halo survives the round cut.
      const {width}=await sharp(source).metadata(),inset=Math.round(width*0.012);
      pipeline=sharp(await sharp(source).extract({left:inset,top:inset,width:width-2*inset,height:width-2*inset}).toBuffer()).resize(size,size,{fit:"cover",kernel:"lanczos3"});
    }
    const {data}=await pipeline.ensureAlpha().raw().toBuffer({resolveWithObject:true});
    roundCut(data,size);
    const bin=await indexed(data,size);
    await writeFile(path.join(folder,stem+".bin"),bin);
    await sharp(decoded(bin,size),{raw:{width:size,height:size,channels:4}}).png().toFile(path.join(folder,stem+".png"));
  }
}
await writeFile(path.join(root,"catalog.json"),JSON.stringify({sizes,names,skins:sets},null,2)+"\n");
console.log("Generated 2 complete free skins: 22 LVGL I8 binaries and previews.");
