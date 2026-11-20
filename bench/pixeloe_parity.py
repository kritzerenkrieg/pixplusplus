"""Phase G parity check (G3, G5): Pix++ `--method pixeloe` against pixeloe.torch.

Both sides get the same input: each source is resized with Pillow's Lanczos to 4x the output
size (long side S), as G7 does, so Pix++ does no resampling of its own.
Default PixelOE options, pixel size 4, no quantisation (so G10 doesn't apply).

usage:
  pixeloe_parity.py prep    SRC_DIR WORK [--sizes 128 512]   resize sources into WORK/in/
  pixeloe_parity.py torch   WORK                             pixeloe.torch -> WORK/torch/ (heavy, A5)
  pixeloe_parity.py pixplusplus   WORK PIXPLUSPLUS_BIN                   pixplusplus -> WORK/pixplusplus/
  pixeloe_parity.py compare WORK                             G5 table (markdown) to stdout

Each torch run is a separate process so its peak RSS is its own. Timings exclude imports.
"""
import argparse, json, resource, subprocess, sys, time
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
PS = 4


def out_size(w, h, s):
    """Pix++'s -s rule (src/main.rs): long side s, aspect kept, rounded."""
    return (s, max(1, round(h * s / w))) if w >= h else (max(1, round(w * s / h)), s)


def cmd_prep(a):
    work = Path(a.work)
    (work / "in").mkdir(parents=True, exist_ok=True)
    srcs = sorted(p for p in Path(a.src).iterdir() if p.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp"})
    for p in srcs:
        img = Image.open(p).convert("RGB")
        for s in a.sizes:
            ow, oh = out_size(*img.size, s)
            # p.name keeps scene.jpg and scene.png apart; the trailing _<size> still parses.
            img.resize((ow * PS, oh * PS), Image.LANCZOS).save(work / "in" / f"{p.name}_{s}.png")
            print(p.stem, s, (ow, oh))


def torch_one(inp, out):
    """Child process: one pixelize run; prints JSON stats."""
    import torch
    from pixeloe.torch.pixelize import pixelize

    torch.set_grad_enabled(False)
    x = torch.from_numpy(np.asarray(Image.open(inp).convert("RGB"), np.float32) / 255.0)
    x = x.permute(2, 0, 1)[None].contiguous()
    t = time.perf_counter()
    y = pixelize(x, pixel_size=PS, no_post_upscale=True, backend="torch")
    ms = (time.perf_counter() - t) * 1000
    arr = (y[0].permute(1, 2, 0).clamp(0, 1).numpy() * 255).round().astype(np.uint8)
    Image.fromarray(arr).save(out)
    print(json.dumps({"ms": ms, "peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                      "threads": torch.get_num_threads()}))


def runs(work, sub):
    (work / sub).mkdir(exist_ok=True)
    return [(p, work / sub / p.name, work / sub / (p.stem + ".json")) for p in sorted((work / "in").glob("*.png"))]


def cmd_torch(a):
    for inp, out, js in runs(Path(a.work), "torch"):
        r = subprocess.run([sys.executable, __file__, "_torch_one", str(inp), str(out)],
                           check=True, capture_output=True, text=True)
        js.write_text(r.stdout.strip().splitlines()[-1])
        print(inp.name, js.read_text())


def cmd_pixplusplus(a):
    for inp, out, js in runs(Path(a.work), "pixplusplus"):
        s = int(inp.stem.rsplit("_", 1)[1])
        subprocess.run([a.bin, str(inp), "-o", str(out), "-s", str(s), "--method", "pixeloe",
                        "--input-factor", str(PS), "--stats", str(js)], check=True)


def srgb_to_oklab(rgb8):
    sys.path.insert(0, str(HERE))
    from metrics import srgb_to_oklab as f
    return f(rgb8)


def cmd_compare(a):
    work = Path(a.work)
    rows, worst = [], 0.0
    print("| image | size | mean ΔE | max ΔE | identical | torch ms | pixplusplus ms | torch peak MiB | pixplusplus peak MiB |")
    print("|---|---|---|---|---|---|---|---|---|")
    for inp in sorted((work / "in").glob("*.png")):
        t = np.asarray(Image.open(work / "torch" / inp.name).convert("RGB"))
        r = np.asarray(Image.open(work / "pixplusplus" / inp.name).convert("RGB"))
        assert t.shape == r.shape, (inp.name, t.shape, r.shape)
        de = np.linalg.norm(srgb_to_oklab(t) - srgb_to_oklab(r), axis=-1) * 100
        same = (t == r).all(-1).mean() * 100
        tj = json.loads((work / "torch" / (inp.stem + ".json")).read_text())
        pj = json.loads((work / "pixplusplus" / (inp.stem + ".json")).read_text())
        name, s = inp.stem.rsplit("_", 1)
        ok = de.mean() <= 0.5 and same >= 99.0
        worst = max(worst, de.mean())
        rows.append(ok)
        print(f"| {name} | {s} | {de.mean():.3f} | {de.max():.1f} | {same:.2f} % | {tj['ms']:.0f} | "
              f"{pj['stats']['timings']['total_ms']:.0f} | {tj['peak_rss_kib'] / 1024:.0f} | "
              f"{pj['peak_rss_kib'] / 1024:.0f} |{'' if ok else ' **over G5**'}")
    print(f"\n{sum(rows)}/{len(rows)} within G5 (mean ΔE ≤ 0.5, ≥ 99 % identical); worst mean ΔE {worst:.3f}")
    if not rows:
        sys.exit("no images compared: run the prep step first")
    sys.exit(0 if all(rows) else 1)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "_torch_one":
        torch_one(sys.argv[2], sys.argv[3])
        sys.exit()
    ap = argparse.ArgumentParser()
    sp = ap.add_subparsers(dest="cmd", required=True)
    p = sp.add_parser("prep"); p.add_argument("src"); p.add_argument("work")
    p.add_argument("--sizes", type=int, nargs="+", default=[128, 512])
    sp.add_parser("torch").add_argument("work")
    p = sp.add_parser("pixplusplus"); p.add_argument("work"); p.add_argument("bin")
    sp.add_parser("compare").add_argument("work")
    a = ap.parse_args()
    {"prep": cmd_prep, "torch": cmd_torch, "pixplusplus": cmd_pixplusplus, "compare": cmd_compare}[a.cmd](a)
