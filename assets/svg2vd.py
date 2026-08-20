#!/usr/bin/env python3
"""
Convert an SVG to an Android Vector Drawable XML.
Handles <rect>, <polygon>, and <path> elements (directly or nested in <g>).
"""

import re
import sys
from lxml import etree

NAMED_COLORS = {
    'white': '#FFFFFF',
    'black': '#000000',
    'none': 'none',
    'transparent': 'none',
}

def normalize_color(c):
    if c is None:
        return None
    return NAMED_COLORS.get(c.lower(), c)

def fmt(v):
    s = f'{v:.4f}'.rstrip('0').rstrip('.')
    return s if s and s != '-' else '0'

def scale_xy(x, y, fit, xoff, yoff):
    return x * fit + xoff, y * fit + yoff

def rect_to_path(x, y, w, h, rx, ry, fit, xoff, yoff):
    def f(px, py):
        sx, sy = scale_xy(px, py, fit, xoff, yoff)
        return f'{fmt(sx)},{fmt(sy)}'

    if rx <= 0 and ry <= 0:
        return (f'M{f(x,y)} L{f(x+w,y)} L{f(x+w,y+h)} L{f(x,y+h)} Z')

    rx = min(rx, w / 2)
    ry = min(ry, h / 2)
    return ' '.join([
        f'M{f(x+rx, y)}',
        f'L{f(x+w-rx, y)}',
        f'Q{f(x+w, y)} {f(x+w, y+ry)}',
        f'L{f(x+w, y+h-ry)}',
        f'Q{f(x+w, y+h)} {f(x+w-rx, y+h)}',
        f'L{f(x+rx, y+h)}',
        f'Q{f(x, y+h)} {f(x, y+h-ry)}',
        f'L{f(x, y+ry)}',
        f'Q{f(x, y)} {f(x+rx, y)}',
        'Z',
    ])

def polygon_to_path(points_str, fit, xoff, yoff):
    vals = [float(v) for v in re.findall(r'[-+]?\d*\.?\d+', points_str)]
    pairs = [(vals[i], vals[i + 1]) for i in range(0, len(vals) - 1, 2)]
    parts = []
    for i, (x, y) in enumerate(pairs):
        sx, sy = scale_xy(x, y, fit, xoff, yoff)
        parts.append(f'{"M" if i == 0 else "L"}{fmt(sx)},{fmt(sy)}')
    parts.append('Z')
    return ' '.join(parts)

def path_to_vd(d, fit, xoff, yoff):
    tokens = re.findall(
        r'[MmCcLlZzQqHhVv]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?', d)
    nargs = {'M': 2, 'm': 2, 'L': 2, 'l': 2,
             'C': 6, 'c': 6, 'Q': 4, 'q': 4,
             'H': 1, 'h': 1, 'V': 1, 'v': 1,
             'Z': 0, 'z': 0}
    i, cmd = 0, None
    parts = []
    cx, cy = 0.0, 0.0  # current absolute position for H/V/relative
    while i < len(tokens):
        t = tokens[i]
        if t in nargs:
            cmd = t
            i += 1
            if nargs[cmd] == 0:
                parts.append('Z')
                cmd = None
            continue
        if cmd is None:
            i += 1
            continue
        n = nargs[cmd]
        if i + n > len(tokens):
            break
        a = [float(tokens[i + k]) for k in range(n)]
        i += n

        if cmd == 'M':
            cx, cy = a[0], a[1]
            sx, sy = scale_xy(cx, cy, fit, xoff, yoff)
            parts.append(f'M{fmt(sx)},{fmt(sy)}')
            cmd = 'L'
        elif cmd == 'm':
            cx, cy = cx + a[0], cy + a[1]
            sx, sy = scale_xy(cx, cy, fit, xoff, yoff)
            parts.append(f'M{fmt(sx)},{fmt(sy)}')
            cmd = 'l'
        elif cmd == 'L':
            cx, cy = a[0], a[1]
            sx, sy = scale_xy(cx, cy, fit, xoff, yoff)
            parts.append(f'L{fmt(sx)},{fmt(sy)}')
        elif cmd == 'l':
            sx, sy = a[0] * fit, a[1] * fit
            cx, cy = cx + a[0], cy + a[1]
            parts.append(f'l{fmt(sx)},{fmt(sy)}')
        elif cmd == 'H':
            cx = a[0]
            sx, sy = scale_xy(cx, cy, fit, xoff, yoff)
            parts.append(f'L{fmt(sx)},{fmt(sy)}')
        elif cmd == 'h':
            cx += a[0]
            parts.append(f'l{fmt(a[0]*fit)},0')
        elif cmd == 'V':
            cy = a[0]
            sx, sy = scale_xy(cx, cy, fit, xoff, yoff)
            parts.append(f'L{fmt(sx)},{fmt(sy)}')
        elif cmd == 'v':
            cy += a[0]
            parts.append(f'l0,{fmt(a[0]*fit)}')
        elif cmd == 'C':
            pts = []
            for j in range(0, 6, 2):
                sx, sy = scale_xy(a[j], a[j+1], fit, xoff, yoff)
                pts += [fmt(sx), fmt(sy)]
            cx, cy = a[4], a[5]
            parts.append(f'C{",".join(pts)}')
        elif cmd == 'c':
            pts = []
            for j in range(0, 6, 2):
                pts += [fmt(a[j] * fit), fmt(a[j+1] * fit)]
            cx, cy = cx + a[4], cy + a[5]
            parts.append(f'c{",".join(pts)}')
        elif cmd == 'Q':
            pts = []
            for j in range(0, 4, 2):
                sx, sy = scale_xy(a[j], a[j+1], fit, xoff, yoff)
                pts += [fmt(sx), fmt(sy)]
            cx, cy = a[2], a[3]
            parts.append(f'Q{",".join(pts)}')
        elif cmd == 'q':
            pts = []
            for j in range(0, 4, 2):
                pts += [fmt(a[j] * fit), fmt(a[j+1] * fit)]
            cx, cy = cx + a[2], cy + a[3]
            parts.append(f'q{",".join(pts)}')

    return ' '.join(parts)

def collect_shapes(node, NS, parent_fill='#000000'):
    """Recursively yield (resolved_fill, element) for drawable shapes."""
    for child in node:
        tag = child.tag
        raw_fill = child.get('fill')
        fill = normalize_color(raw_fill) if raw_fill is not None else parent_fill
        if tag == f'{{{NS}}}g':
            yield from collect_shapes(child, NS, fill)
        elif tag in (f'{{{NS}}}rect', f'{{{NS}}}polygon', f'{{{NS}}}path'):
            yield fill, child

def main():
    if len(sys.argv) < 3:
        print(f'Usage: {sys.argv[0]} input.svg output.xml', file=sys.stderr)
        sys.exit(1)

    svg_file, out_file = sys.argv[1], sys.argv[2]
    VIEWPORT = 108.0

    tree = etree.parse(svg_file)
    root = tree.getroot()
    NS = 'http://www.w3.org/2000/svg'

    vb = root.get('viewBox', '').split()
    svgw = float(vb[2]) if len(vb) >= 4 else float(root.get('width', VIEWPORT))
    svgh = float(vb[3]) if len(vb) >= 4 else float(root.get('height', VIEWPORT))

    fit = VIEWPORT / max(svgw, svgh)
    xoff = (VIEWPORT - svgw * fit) / 2
    yoff = (VIEWPORT - svgh * fit) / 2

    path_elems = []
    for fill, elem in collect_shapes(root, NS):
        if fill == 'none':
            continue
        tag = elem.tag
        if tag == f'{{{NS}}}rect':
            x = float(elem.get('x', 0))
            y = float(elem.get('y', 0))
            w = float(elem.get('width', 0))
            h = float(elem.get('height', 0))
            rx = float(elem.get('rx', elem.get('ry', 0)))
            ry = float(elem.get('ry', rx))
            pd = rect_to_path(x, y, w, h, rx, ry, fit, xoff, yoff)
        elif tag == f'{{{NS}}}polygon':
            pd = polygon_to_path(elem.get('points', ''), fit, xoff, yoff)
        elif tag == f'{{{NS}}}path':
            pd = path_to_vd(elem.get('d', ''), fit, xoff, yoff)
        else:
            continue
        path_elems.append(
            f'    <path\n'
            f'        android:fillColor="{fill}"\n'
            f'        android:pathData="{pd}" />'
        )

    vp = int(VIEWPORT)
    xml = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
        f'    android:width="{vp}dp"\n'
        f'    android:height="{vp}dp"\n'
        f'    android:viewportWidth="{vp}"\n'
        f'    android:viewportHeight="{vp}">\n'
        + '\n'.join(path_elems) + '\n'
        '</vector>\n'
    )

    with open(out_file, 'w', encoding='utf-8') as f:
        f.write(xml)

if __name__ == '__main__':
    main()
