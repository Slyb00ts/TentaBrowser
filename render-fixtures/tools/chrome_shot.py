"""Takes a headless Chrome screenshot of a page and reads PNG pixels without
third-party modules.

Usage: chrome_shot.py DIR WIDTH HEIGHT   (screenshots DIR/parity.html to DIR/chrome.png)
"""
import subprocess, sys, zlib, struct, json
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
def shot(url, out, w, h):
    subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
                    f"--window-size={w},{h}", "--virtual-time-budget=4000", "--allow-file-access-from-files",
                    f"--screenshot={out}", url], capture_output=True, timeout=60)

def read_png(path):
    data = open(path, "rb").read()
    pos = 8
    idat = b""
    while pos < len(data):
        ln, = struct.unpack(">I", data[pos:pos+4]); kind = data[pos+4:pos+8]
        body = data[pos+8:pos+8+ln]; pos += 12 + ln
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", body[:10])
        elif kind == b"IDAT":
            idat += body
    assert depth == 8 and ctype in (2, 6), (depth, ctype)
    bpp = 3 if ctype == 2 else 4
    raw = zlib.decompress(idat)
    stride = w * bpp
    rows = []
    prev = bytearray(stride)
    i = 0
    for y in range(h):
        f = raw[i]; line = bytearray(raw[i+1:i+1+stride]); i += 1 + stride
        for x in range(stride):
            a = line[x-bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x-bpp] if x >= bpp else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + b) & 255
            elif f == 3: line[x] = (line[x] + ((a + b) >> 1)) & 255
            elif f == 4:
                p = a + b - c; pa = abs(p-a); pb = abs(p-b); pc = abs(p-c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        rows.append(line); prev = line
    return w, h, bpp, rows

if __name__ == "__main__":
    d = sys.argv[1]
    w, h = int(sys.argv[2]), int(sys.argv[3])
    shot("file://" + d + "/parity.html", d + "/chrome.png", w, h)
    W, H, bpp, rows = read_png(d + "/chrome.png")
    print("screenshot", W, H)
