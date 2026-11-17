"""Build a compact, labeled comparison page: per image, the source plus 3 methods (E8) x 2 sizes.

No rating controls. Images are embedded, so the page is one self-contained HTML file.
Click a tile to view it full size. Output is local only (bench/out/ is git-ignored).

usage: showcase.py RUN_DIR SRC_DIR [-o bench/out/showcase/index.html] [--sizes 128 512] [--k 16]
  RUN_DIR  a run_bench.py output folder (results/<image>_<size>_k<k>_<method>.png)
  SRC_DIR  the source images, with an optional SOURCES.json ({stem: {post, source}})
"""
import argparse, base64, html, io, json
from pathlib import Path
from PIL import Image

METHODS = ["pixeloe", "pia", "pia_imp"]  # E8: the three methods
INFO = {"pixeloe": "PixelOE + Oklab k-means", "pia": "PIA", "pia_imp": "PIA + saliency map"}

ap = argparse.ArgumentParser()
ap.add_argument("run")
ap.add_argument("src")
ap.add_argument("-o", "--out", default=str(Path(__file__).resolve().parent / "out" / "showcase" / "index.html"))
ap.add_argument("--sizes", type=int, nargs="+", default=[128, 512])
ap.add_argument("--k", type=int, default=16)
a = ap.parse_args()
run, src = Path(a.run), Path(a.src)
sources = json.loads((src / "SOURCES.json").read_text()) if (src / "SOURCES.json").exists() else {}


def uri(path, max_side=None, fmt="PNG"):
    im = Image.open(path).convert("RGB")
    if max_side:
        im.thumbnail((max_side, max_side), Image.Resampling.LANCZOS)
    b = io.BytesIO()
    im.save(b, fmt, optimize=True) if fmt == "PNG" else im.save(b, fmt, quality=85)
    return f"data:image/{fmt.lower()};base64," + base64.b64encode(b.getvalue()).decode()


rows = []
for img in sorted(p for p in src.iterdir() if p.suffix.lower() in (".png", ".jpg", ".jpeg", ".webp")):
    stem = img.stem
    meta = sources.get(stem, {})
    link = meta.get("post", "")
    cells = [f'<div class="lab"><b>{html.escape(stem)}</b>'
             + (f' <a href="{html.escape(link)}" target="_blank" rel="noopener">post</a>' if link else "")
             + "</div>"]
    head = [f'<div class="src" style="grid-row:span {len(a.sizes)}"><img src="{uri(img, 640, "JPEG")}" alt="{stem} source"></div>']
    body = []
    for s in a.sizes:
        body.append(f'<div class="sz">{s}</div>')
        for m in METHODS:
            p = run / "results" / f"{stem}_{s}_k{a.k}_{m}.png"
            body.append(f'<figure><img class="px" src="{uri(p)}" alt="{m} {s}"></figure>' if p.exists()
                        else '<figure class="miss">missing</figure>')
    rows.append("".join(cells) + '<div class="row">' + "".join(head) + "".join(body) + "</div>")

hdr = '<div class="row hdr"><div></div><div></div>' + "".join(
    f'<div><b>{m}</b><br><span class="mut">{INFO[m]}</span></div>' for m in METHODS) + "</div>"

page = f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>PIARU Showcase</title>
<style>
:root{{--bg:#f6f5f2;--fg:#1d1d1f;--mut:#6b6b70;--line:#dcdbd6}}
@media (prefers-color-scheme:dark){{:root{{--bg:#141416;--fg:#ececef;--mut:#9a9aa2;--line:#34343a}}}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--fg);font:13px/1.35 system-ui,sans-serif}}
main{{max-width:1400px;margin:0 auto;padding:12px 16px}}h1{{font-size:16px;margin:0 0 2px}}.mut{{color:var(--mut)}}
.row{{display:grid;grid-template-columns:minmax(120px,1.4fr) 34px repeat(3,1fr);gap:4px;align-items:center}}
.hdr{{position:sticky;top:0;background:var(--bg);padding:6px 0;z-index:2;border-bottom:1px solid var(--line);align-items:end}}
.lab{{margin:14px 0 4px}}.lab a{{color:var(--mut);margin-left:6px}}
.src img,figure img{{width:100%;height:auto;display:block;border-radius:3px;cursor:zoom-in}}
.px{{image-rendering:pixelated;image-rendering:crisp-edges}}
figure{{margin:0}}.miss{{color:var(--mut);text-align:center}}
.sz{{writing-mode:vertical-rl;transform:rotate(180deg);text-align:center;color:var(--mut)}}
#zoom{{position:fixed;inset:0;background:rgba(0,0,0,.85);display:none;align-items:center;justify-content:center;z-index:9;cursor:zoom-out}}
#zoom img{{max-width:96vw;max-height:96vh;image-rendering:pixelated}}
</style></head><body><main>
<h1>PIARU showcase</h1><div class="mut">K = {a.k}, output width {' / '.join(map(str, a.sizes))} px. Click any tile to enlarge.</div>
{hdr}{''.join(rows)}
</main><div id="zoom"><img alt=""></div>
<script>
const z=document.getElementById('zoom'),zi=z.querySelector('img');
document.querySelectorAll('main img').forEach(i=>i.onclick=()=>{{zi.src=i.src;z.style.display='flex'}});
z.onclick=()=>z.style.display='none';
</script></body></html>"""
out = Path(a.out)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(page)
print(f"wrote {out} ({len(rows)} images, {out.stat().st_size / 1e6:.1f} MB)")
