#!/usr/bin/env python3
import html
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BASE = ["#1e1e1e", "#d2463c", "#2e9d5b", "#c7851a", "#3a6fd8", "#a626a4", "#1f9aa8", "#d0d0d0"]
BRIGHT = ["#7f7f7f", "#ff6b61", "#4cc47f", "#e8a93a", "#6b95ff", "#d066cf", "#3fc3d2", "#ffffff"]
FOREGROUND = "#e6e6e6"
BACKGROUND = "#161616"
SGR = re.compile(r"\x1b\[([0-9;]*)m")
OTHER_ESCAPES = re.compile(r"\x1b\[[0-9;?]*[A-Za-ln-z]")


def xterm_256(n):
    if n < 8:
        return BASE[n]
    if n < 16:
        return BRIGHT[n - 8]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (steps[n // 36], steps[(n // 6) % 6], steps[n % 6])
    level = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (level, level, level)


def apply(codes, style):
    values = [int(c) if c else 0 for c in codes.split(";")] if codes else [0]
    i = 0
    while i < len(values):
        v = values[i]
        if v == 0:
            style.clear()
        elif v == 1:
            style["bold"] = True
        elif v == 2:
            style["dim"] = True
        elif v == 3:
            style["italic"] = True
        elif v == 4:
            style["underline"] = True
        elif v == 22:
            style.pop("bold", None)
            style.pop("dim", None)
        elif v == 23:
            style.pop("italic", None)
        elif v == 24:
            style.pop("underline", None)
        elif 30 <= v <= 37:
            style["fg"] = BASE[v - 30]
        elif 90 <= v <= 97:
            style["fg"] = BRIGHT[v - 90]
        elif 40 <= v <= 47:
            style["bg"] = BASE[v - 40]
        elif 100 <= v <= 107:
            style["bg"] = BRIGHT[v - 100]
        elif v == 39:
            style.pop("fg", None)
        elif v == 49:
            style.pop("bg", None)
        elif v in (38, 48) and i + 1 < len(values):
            key = "fg" if v == 38 else "bg"
            if values[i + 1] == 5 and i + 2 < len(values):
                style[key] = xterm_256(values[i + 2])
                i += 2
            elif values[i + 1] == 2 and i + 4 < len(values):
                style[key] = "#%02x%02x%02x" % tuple(values[i + 2:i + 5])
                i += 4
        i += 1


def css(style):
    rules = []
    if "fg" in style:
        rules.append("color:" + style["fg"])
    if "bg" in style:
        rules.append("background:" + style["bg"])
    if style.get("bold"):
        rules.append("font-weight:700")
    if style.get("dim"):
        rules.append("opacity:.6")
    if style.get("italic"):
        rules.append("font-style:italic")
    if style.get("underline"):
        rules.append("text-decoration:underline")
    return ";".join(rules)


def to_html(text):
    text = OTHER_ESCAPES.sub("", text.replace("\r", ""))
    style, out, last = {}, [], 0
    for match in SGR.finditer(text):
        chunk = text[last:match.start()]
        if chunk:
            out.append('<span style="%s">%s</span>' % (css(style), html.escape(chunk)))
        apply(match.group(1), style)
        last = match.end()
    out.append('<span style="%s">%s</span>' % (css(style), html.escape(text[last:])))
    return "".join(out)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: render-frame.py <frame.ansi> <frame.png>")
    source, target = Path(sys.argv[1]), Path(sys.argv[2]).resolve()
    text = source.read_text(errors="replace").rstrip("\n")
    lines = text.count("\n") + 1
    width, height = 1280, max(800, 48 + lines * 19)
    page = (
        "<!doctype html><meta charset=utf-8><body style=\"margin:0;background:%s\">"
        "<pre style=\"margin:0;padding:24px;font:13px/19px 'DejaVu Sans Mono',Menlo,monospace;color:%s;white-space:pre\">%s</pre>"
    ) % (BACKGROUND, FOREGROUND, to_html(text))
    chrome = shutil.which("google-chrome") or shutil.which("chromium")
    if not chrome:
        sys.exit("render-frame.py: no headless chrome on PATH")
    with tempfile.TemporaryDirectory() as scratch:
        page_path = Path(scratch) / "frame.html"
        page_path.write_text(page)
        subprocess.run(
            [chrome, "--headless=new", "--disable-gpu", "--hide-scrollbars",
             "--user-data-dir=" + scratch + "/profile",
             "--window-size=%d,%d" % (width, height),
             "--screenshot=" + str(target), page_path.as_uri()],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    print(target)


if __name__ == "__main__":
    main()
