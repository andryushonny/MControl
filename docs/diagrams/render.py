"""Render the README diagrams: python docs/diagrams/render.py  (needs node/npm and Chrome or Chromium)."""
import os, shutil, subprocess, sys, tempfile, re

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "img")
if not os.path.isdir(os.path.join(HERE, "node_modules")):
    subprocess.run(["npm", "install", "--silent"], cwd=HERE, check=True)
chrome = next((c for c in ("google-chrome", "chromium", "chromium-browser", "chrome") if shutil.which(c)), None) or sys.exit("no Chrome")
names = re.findall(r"^D\.(\w+) = ", open(os.path.join(HERE, "diagrams.js")).read(), re.M)
fonts = os.path.join(HERE, "node_modules", "@fontsource", "caveat", "files")
os.makedirs(OUT, exist_ok=True)
for name in sys.argv[1:] or names:
    js = open(os.path.join(HERE, "diagrams.js")).read()
    w, h = map(int, re.search(rf"D\.{name} = \(\) => {{\s*const B = board\((\d+), (\d+)", js).groups())
    page = f"""<!doctype html><meta charset=utf-8><style>
@font-face {{ font-family: Caveat; font-weight: 500; src: url(file://{fonts}/caveat-latin-500-normal.woff2); }}
@font-face {{ font-family: Caveat; font-weight: 700; src: url(file://{fonts}/caveat-latin-700-normal.woff2); }}
html, body {{ margin: 0; background: #fbfaf6; }}
body {{ background-image: radial-gradient(#d9d6cc 1.2px, transparent 1.2px); background-size: 26px 26px; }}
</style><svg xmlns="http://www.w3.org/2000/svg"></svg>
<script src="file://{HERE}/node_modules/roughjs/bundled/rough.js"></script>
<script src="file://{HERE}/board.js"></script><script src="file://{HERE}/diagrams.js"></script>
<script>document.fonts.load("500 20px Caveat").then(() => document.fonts.load("700 20px Caveat")).then(() => {{ D.{name}(); }});</script>"""
    html = os.path.join(tempfile.gettempdir(), f"mc-diagram-{name}.html")
    open(html, "w").write(page)
    png = os.path.join(OUT, f"{name}.png")
    subprocess.run([chrome, "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--allow-file-access-from-files",
                    "--virtual-time-budget=3000", "--force-device-scale-factor=1.5", f"--window-size={w},{h}", f"--screenshot={png}",
                    "file://" + html], check=True, capture_output=True)
    try:
        from PIL import Image
        Image.open(png).convert("RGB").quantize(colors=96, method=Image.Quantize.MEDIANCUT).save(png, optimize=True)
    except ImportError:
        pass
    print(png)
