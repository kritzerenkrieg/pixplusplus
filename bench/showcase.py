"""Build a compact, labeled comparison page: per image, the source plus 3 methods (E8) x 2 sizes.

No rating controls. Images are embedded, so the page is one self-contained HTML file.
Click a tile to view it full size. Output is local only (bench/out/ is git-ignored).
Missing outputs are made by running the Pix++ binary; existing ones are reused. Runs at 512
are heavier than the smoke run, so do those on a remote machine (A5) and copy RUN_DIR back.

usage: showcase.py SRC_DIR [--run bench/out/showcase/runs] [--bin target/release/pixplusplus]
                   [-o bench/out/showcase/index.html] [--sizes 128 512] [--k 16]
  SRC_DIR  the source images, with an optional SOURCES.json ({stem: {post, source}})
  --run    output folder (results/<image>_<size>_k<k>_<method>.png)
"""
import argparse, base64, html, io, json, subprocess
from pathlib import Path
from PIL import Image

METHODS = ["pixeloe", "pia", "pia_imp"]  # E8: the three methods
INFO = {"pixeloe": "PixelOE (k-means)", "pia": "PIA", "pia_imp": "PIA + saliency map"}
FLAGS = {"pixeloe": ["--method", "pixeloe"], "pia": ["--importance", "none"], "pia_imp": ["--importance", "auto"]}
OUT = Path(__file__).resolve().parent / "out" / "showcase"

ap = argparse.ArgumentParser()
ap.add_argument("src")
ap.add_argument("--run", default=str(OUT / "runs"))
ap.add_argument("--bin", default=str(Path(__file__).resolve().parent.parent / "target" / "release" / "pixplusplus"))
ap.add_argument("-o", "--out", default=str(OUT / "index.html"))
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
imgs = sorted(p for p in src.iterdir() if p.suffix.lower() in (".png", ".jpg", ".jpeg", ".webp"))
stems = [p.stem for p in imgs]
for img in imgs:
    stem = img.stem
    # Cache key: the stem, or the full name when two sources share a stem (scene.jpg, scene.png).
    key = stem if stems.count(stem) == 1 else img.name
    meta = sources.get(stem, {})
    link = meta.get("post", "")
    cells = [f'<div class="lab"><b>{html.escape(stem)}</b>'
             + (f' <a href="{html.escape(link)}" target="_blank" rel="noopener">post</a>' if link else "")
             + "</div>"]
    head = [f'<div class="src" style="grid-row:span {len(a.sizes)}"><img src="{uri(img, 640, "JPEG")}" alt="{html.escape(stem, quote=True)} source"></div>']
    body = []
    for s in a.sizes:
        body.append(f'<div class="sz">{s}</div>')
        for m in METHODS:
            p = run / "results" / f"{key}_{s}_k{a.k}_{m}.png"
            if not p.exists():
                p.parent.mkdir(parents=True, exist_ok=True)
                subprocess.run([a.bin, str(img), "-o", str(p), "-s", str(s), "-k", str(a.k), *FLAGS[m]], check=True)
            body.append(f'<figure><img class="px" src="{uri(p)}" alt="{m} {s}"></figure>' if p.exists()
                        else '<figure class="miss">missing</figure>')
    rows.append("".join(cells) + '<div class="row">' + "".join(head) + "".join(body) + "</div>")

hdr = '<div class="row hdr"><div></div><div></div>' + "".join(
    f'<div><b>{m}</b><br><span class="mut">{INFO[m]}</span></div>' for m in METHODS) + "</div>"

page = f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Pix++ Showcase</title>
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
#zoom{{border:0;padding:0;background:transparent;max-width:100vw;max-height:100vh;cursor:zoom-out}}
#zoom::backdrop{{background:rgba(0,0,0,.85)}}
#zoom img{{max-width:96vw;max-height:96vh;image-rendering:pixelated}}
.zb{{all:unset;display:block;width:100%;cursor:zoom-in}}.zb:focus-visible{{outline:2px solid #6af;outline-offset:2px}}
</style></head><body><main>
<h1>Pix++ showcase</h1><div class="mut">K = {a.k}, output width {' / '.join(map(str, a.sizes))} px. Click a tile (or Tab to it and press Enter) to enlarge; Esc closes.</div>
{hdr}{''.join(rows)}
</main><dialog id="zoom" aria-label="Enlarged image"><button type="button" class="zb" aria-label="Close"><img alt=""></button></dialog>
<script>
// A native modal <dialog>: the browser keeps Tab inside it, closes it on Esc and returns focus
// to the tile that opened it. The image is the dialog's one button, so Enter, Space or a click
// closes it too.
const z=document.getElementById('zoom'),zb=z.querySelector('button'),zi=z.querySelector('img');let from=null;
document.querySelectorAll('main img').forEach(i=>{{
  if(z.contains(i))return;
  const b=document.createElement('button');b.className='zb';b.type='button';
  b.setAttribute('aria-label','Enlarge '+(i.alt||'image'));
  i.replaceWith(b);b.append(i);
  b.onclick=()=>{{from=b;zi.src=i.src;zi.alt=i.alt;z.showModal();zb.focus()}};
}});
zb.onclick=()=>z.close();
z.onclick=e=>{{if(e.target===z)z.close()}};
z.addEventListener('close',()=>{{if(from)from.focus()}});
</script></body></html>"""
out = Path(a.out)
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(page, encoding="utf-8")
print(f"wrote {out} ({len(rows)} images, {out.stat().st_size / 1e6:.1f} MB)")
