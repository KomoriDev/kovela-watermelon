import json
import shutil
import subprocess
import zipfile
from pathlib import Path

root = Path(__file__).resolve().parent
manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
if manifest["api_level"] != 4 or manifest["wasi_version"] != 3:
    raise SystemExit("This plugin requires AstroBox API level 4 / WASI 3")
subprocess.run(["cargo", "build", "--release", "--target", "wasm32-wasip2", "--locked"], cwd=root, check=True)
output = root / "dist" / "watermelon"
output.mkdir(parents=True, exist_ok=True)
(output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
shutil.copy2(root / "target" / "wasm32-wasip2" / "release" / manifest["entry"], output / manifest["entry"])
shutil.copy2(root / manifest["icon"], output / manifest["icon"])
archive_path = output / "Kovela-Watermelon.abp"
with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
    for name in ["manifest.json", manifest["entry"], manifest["icon"]]:
        archive.write(output / name, name)
print("Built", output, "and", archive_path)
