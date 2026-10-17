import {readFile,copyFile} from "node:fs/promises";
import {fileURLToPath} from "node:url";
import path from "node:path";
const root=fileURLToPath(new URL("../",import.meta.url));
const game=path.resolve(root,process.argv[2]||"../watermelon");
const manifest=JSON.parse(await readFile(path.join(game,"src/manifest.json"),"utf8"));
if(manifest.package!=="com.komoridev.watermelon")throw new Error("Target must be Watermelon");
const source=path.join(game,"dist",`${manifest.package}.release.${manifest.versionName}.rpk`);
const catalogPath=path.join(root,"manifest_v2.json");
const catalog=JSON.parse(await readFile(catalogPath,"utf8"));
for(const [model,download] of Object.entries(catalog.downloads)){
  const name=`downloads/${manifest.package}.release.${manifest.versionName}-${model}.rpk`;
  await copyFile(source,path.join(root,name));
  download.version=manifest.versionName;download.versionCode=manifest.versionCode;download.file_name=name;
  if(!download.updatelogs.some(entry=>entry.version===manifest.versionName))download.updatelogs.push({version:manifest.versionName,content:"- 🍉 独立 AstroBox V4 换肤插件，两套内置皮肤免费使用\n- 📲 手环图片传输、预览与恢复默认全部免费\n- 🎨 自定义图片在插件内按爱发电绑定账号验证，旧购买商品继续有效"});
}
const {writeFile}=await import("node:fs/promises");
await writeFile(catalogPath,JSON.stringify(catalog,null,2)+"\n");
console.log(`Staged Watermelon ${manifest.versionName} for ${Object.keys(catalog.downloads).length} existing device entries`);
