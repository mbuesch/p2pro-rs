#!/usr/bin/env python3
"""
Convert an SVG to an Android Vector Drawable XML.
Handles <rect>, <circle>, <polygon>, <path>, and <line> elements
(directly or nested in <g>), including inherited fill/stroke styling,
a viewBox with a non-zero origin, and simple linearGradient fills.
"""

import re
import sys
from lxml import etree

NS = 'http://www.w3.org/2000/svg'

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


# ---------------------------------------------------------------- shapes --

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


def circle_to_path(cx, cy, r, fit, xoff, yoff):
    """Two-arc full circle, since Android pathData has no dedicated circle op."""
    scx, scy = scale_xy(cx, cy, fit, xoff, yoff)
    sr = r * fit
    return (f'M{fmt(scx - sr)},{fmt(scy)} '
            f'A{fmt(sr)},{fmt(sr)} 0 1,0 {fmt(scx + sr)},{fmt(scy)} '
            f'A{fmt(sr)},{fmt(sr)} 0 1,0 {fmt(scx - sr)},{fmt(scy)} Z')


def line_to_path(x1, y1, x2, y2, fit, xoff, yoff):
    sx1, sy1 = scale_xy(x1, y1, fit, xoff, yoff)
    sx2, sy2 = scale_xy(x2, y2, fit, xoff, yoff)
    return f'M{fmt(sx1)},{fmt(sy1)} L{fmt(sx2)},{fmt(sy2)}'


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


def path_bbox(d):
    """Approximate (min_x, min_y, max_x, max_y) of a path in raw SVG units,
    including control points. Only used to place objectBoundingBox gradients,
    so a slight overestimate on curves is fine."""
    tokens = re.findall(
        r'[MmCcLlZzQqHhVv]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?', d)
    nargs = {'M': 2, 'm': 2, 'L': 2, 'l': 2,
             'C': 6, 'c': 6, 'Q': 4, 'q': 4,
             'H': 1, 'h': 1, 'V': 1, 'v': 1,
             'Z': 0, 'z': 0}
    i, cmd = 0, None
    cx, cy = 0.0, 0.0
    xs, ys = [], []
    while i < len(tokens):
        t = tokens[i]
        if t in nargs:
            cmd = t
            i += 1
            if nargs[cmd] == 0:
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
            cx, cy = a[0], a[1]; xs.append(cx); ys.append(cy); cmd = 'L'
        elif cmd == 'm':
            cx, cy = cx + a[0], cy + a[1]; xs.append(cx); ys.append(cy); cmd = 'l'
        elif cmd == 'L':
            cx, cy = a[0], a[1]; xs.append(cx); ys.append(cy)
        elif cmd == 'l':
            cx, cy = cx + a[0], cy + a[1]; xs.append(cx); ys.append(cy)
        elif cmd == 'H':
            cx = a[0]; xs.append(cx); ys.append(cy)
        elif cmd == 'h':
            cx += a[0]; xs.append(cx); ys.append(cy)
        elif cmd == 'V':
            cy = a[0]; xs.append(cx); ys.append(cy)
        elif cmd == 'v':
            cy += a[0]; xs.append(cx); ys.append(cy)
        elif cmd == 'C':
            for j in range(0, 6, 2):
                xs.append(a[j]); ys.append(a[j+1])
            cx, cy = a[4], a[5]
        elif cmd == 'c':
            for j in range(0, 6, 2):
                xs.append(cx + a[j]); ys.append(cy + a[j+1])
            cx, cy = cx + a[4], cy + a[5]
        elif cmd == 'Q':
            for j in range(0, 4, 2):
                xs.append(a[j]); ys.append(a[j+1])
            cx, cy = a[2], a[3]
        elif cmd == 'q':
            for j in range(0, 4, 2):
                xs.append(cx + a[j]); ys.append(cy + a[j+1])
            cx, cy = cx + a[2], cy + a[3]
    if not xs:
        return 0.0, 0.0, 0.0, 0.0
    return min(xs), min(ys), max(xs), max(ys)


def shape_bbox(tag, elem):
    """Raw (untransformed) bounding box, used to resolve objectBoundingBox gradients."""
    if tag == f'{{{NS}}}rect':
        x = float(elem.get('x', 0)); y = float(elem.get('y', 0))
        w = float(elem.get('width', 0)); h = float(elem.get('height', 0))
        return x, y, x + w, y + h
    if tag == f'{{{NS}}}circle':
        cx = float(elem.get('cx', 0)); cy = float(elem.get('cy', 0))
        r = float(elem.get('r', 0))
        return cx - r, cy - r, cx + r, cy + r
    if tag == f'{{{NS}}}polygon':
        vals = [float(v) for v in re.findall(r'[-+]?\d*\.?\d+', elem.get('points', ''))]
        xs, ys = vals[0::2], vals[1::2]
        return min(xs), min(ys), max(xs), max(ys)
    if tag == f'{{{NS}}}path':
        return path_bbox(elem.get('d', ''))
    return 0.0, 0.0, 0.0, 0.0


# -------------------------------------------------------------- gradients --

def parse_percent_or_num(s, default):
    if s is None:
        return default
    s = s.strip()
    if s.endswith('%'):
        return float(s[:-1]) / 100.0
    return float(s)


def parse_stop_color(stop):
    c = stop.get('stop-color')
    if c is None:
        style = stop.get('style', '')
        m = re.search(r'stop-color:\s*([^;]+)', style)
        if m:
            c = m.group(1).strip()
    return normalize_color(c) or '#000000'


def collect_gradients(root):
    gradients = {}
    for lg in root.iter(f'{{{NS}}}linearGradient'):
        gid = lg.get('id')
        if not gid:
            continue
        stops = []
        for stop in lg.iter(f'{{{NS}}}stop'):
            offset = parse_percent_or_num(stop.get('offset'), 0.0)
            stops.append((offset, parse_stop_color(stop)))
        gradients[gid] = {
            'x1': lg.get('x1', '0%'), 'y1': lg.get('y1', '0%'),
            'x2': lg.get('x2', '100%'), 'y2': lg.get('y2', '0%'),
            'units': lg.get('gradientUnits', 'objectBoundingBox'),
            'stops': stops,
        }
    return gradients


def gradient_endpoints(grad, bbox, fit, xoff, yoff):
    bx0, by0, bx1, by1 = bbox
    if grad['units'] == 'userSpaceOnUse':
        gx1 = parse_percent_or_num(grad['x1'], 0.0)
        gy1 = parse_percent_or_num(grad['y1'], 0.0)
        gx2 = parse_percent_or_num(grad['x2'], 1.0)
        gy2 = parse_percent_or_num(grad['y2'], 0.0)
    else:
        fx1 = parse_percent_or_num(grad['x1'], 0.0)
        fy1 = parse_percent_or_num(grad['y1'], 0.0)
        fx2 = parse_percent_or_num(grad['x2'], 1.0)
        fy2 = parse_percent_or_num(grad['y2'], 0.0)
        gx1 = bx0 + fx1 * (bx1 - bx0)
        gy1 = by0 + fy1 * (by1 - by0)
        gx2 = bx0 + fx2 * (bx1 - bx0)
        gy2 = by0 + fy2 * (by1 - by0)
    sx1, sy1 = scale_xy(gx1, gy1, fit, xoff, yoff)
    sx2, sy2 = scale_xy(gx2, gy2, fit, xoff, yoff)
    return sx1, sy1, sx2, sy2


# ------------------------------------------------------------- traversal --

def child_style(elem, parent_style):
    style = dict(parent_style)
    for attr in ('fill', 'stroke', 'stroke-width', 'stroke-linecap'):
        v = elem.get(attr)
        if v is not None:
            style[attr] = v
    return style


def collect_shapes(node, parent_style):
    """Recursively yield (style, element) for drawable shapes, propagating
    inherited fill/stroke styling through <g> wrappers."""
    for child in node:
        tag = child.tag
        style = child_style(child, parent_style)
        if tag == f'{{{NS}}}g':
            yield from collect_shapes(child, style)
        elif tag in (f'{{{NS}}}rect', f'{{{NS}}}polygon', f'{{{NS}}}path',
                     f'{{{NS}}}circle', f'{{{NS}}}line'):
            yield style, child


def main():
    if len(sys.argv) < 3:
        print(f'Usage: {sys.argv[0]} input.svg output.xml', file=sys.stderr)
        sys.exit(1)

    svg_file, out_file = sys.argv[1], sys.argv[2]
    VIEWPORT = 108.0

    tree = etree.parse(svg_file)
    root = tree.getroot()

    vb = root.get('viewBox', '').split()
    if len(vb) >= 4:
        svgx, svgy, svgw, svgh = (float(v) for v in vb[:4])
    else:
        svgx = svgy = 0.0
        svgw = float(root.get('width', VIEWPORT))
        svgh = float(root.get('height', VIEWPORT))

    fit = VIEWPORT / max(svgw, svgh)
    # Center the (possibly non-square) content, and shift by the viewBox's
    # own origin so a viewBox like "64 64 384 384" is handled correctly.
    xoff = (VIEWPORT - svgw * fit) / 2 - svgx * fit
    yoff = (VIEWPORT - svgh * fit) / 2 - svgy * fit

    gradients = collect_gradients(root)
    url_re = re.compile(r'^url\(#(.+)\)$')

    root_style = {'fill': '#000000', 'stroke': 'none',
                  'stroke-width': '1', 'stroke-linecap': 'butt'}

    path_elems = []
    for style, elem in collect_shapes(root, root_style):
        tag = elem.tag
        fill_raw = style.get('fill')
        fill = normalize_color(fill_raw)
        stroke = normalize_color(style.get('stroke'))

        # A <line> has no interior, regardless of any inherited fill.
        if tag == f'{{{NS}}}line':
            fill = None

        if (not fill or fill == 'none') and (not stroke or stroke == 'none'):
            continue  # nothing visible to draw

        if tag == f'{{{NS}}}rect':
            x = float(elem.get('x', 0)); y = float(elem.get('y', 0))
            w = float(elem.get('width', 0)); h = float(elem.get('height', 0))
            rx = float(elem.get('rx', elem.get('ry', 0)))
            ry = float(elem.get('ry', rx))
            pd = rect_to_path(x, y, w, h, rx, ry, fit, xoff, yoff)
        elif tag == f'{{{NS}}}circle':
            cx = float(elem.get('cx', 0)); cy = float(elem.get('cy', 0))
            r = float(elem.get('r', 0))
            pd = circle_to_path(cx, cy, r, fit, xoff, yoff)
        elif tag == f'{{{NS}}}polygon':
            pd = polygon_to_path(elem.get('points', ''), fit, xoff, yoff)
        elif tag == f'{{{NS}}}line':
            pd = line_to_path(float(elem.get('x1', 0)), float(elem.get('y1', 0)),
                               float(elem.get('x2', 0)), float(elem.get('y2', 0)),
                               fit, xoff, yoff)
        elif tag == f'{{{NS}}}path':
            pd = path_to_vd(elem.get('d', ''), fit, xoff, yoff)
        else:
            continue

        lines = [f'        android:pathData="{pd}"']
        fill_block = None

        if fill and fill != 'none':
            m = url_re.match(fill_raw.strip()) if fill_raw else None
            if m and m.group(1) in gradients:
                grad = gradients[m.group(1)]
                bbox = shape_bbox(tag, elem)
                sx1, sy1, sx2, sy2 = gradient_endpoints(grad, bbox, fit, xoff, yoff)
                items = '\n'.join(
                    f'                <item android:offset="{fmt(off)}" android:color="{color}" />'
                    for off, color in grad['stops']
                )
                fill_block = (
                    '        <aapt:attr name="android:fillColor">\n'
                    '            <gradient\n'
                    '                android:type="linear"\n'
                    f'                android:startX="{fmt(sx1)}"\n'
                    f'                android:startY="{fmt(sy1)}"\n'
                    f'                android:endX="{fmt(sx2)}"\n'
                    f'                android:endY="{fmt(sy2)}">\n'
                    f'{items}\n'
                    '            </gradient>\n'
                    '        </aapt:attr>'
                )
            else:
                lines.append(f'        android:fillColor="{fill}"')
        # else: omit fillColor entirely -> Android leaves the path unfilled

        if stroke and stroke != 'none':
            sw = float(style.get('stroke-width', 1)) * fit
            cap = style.get('stroke-linecap', 'butt')
            lines.append(f'        android:strokeColor="{stroke}"')
            lines.append(f'        android:strokeWidth="{fmt(sw)}"')
            if cap in ('round', 'square'):
                lines.append(f'        android:strokeLineCap="{cap}"')

        if fill_block:
            path_elems.append('    <path\n' + '\n'.join(lines) + '>\n' + fill_block + '\n    </path>')
        else:
            path_elems.append('    <path\n' + '\n'.join(lines) + ' />')

    vp = int(VIEWPORT)
    needs_aapt = any('aapt:attr' in p for p in path_elems)
    aapt_ns = '\n    xmlns:aapt="http://schemas.android.com/aapt"' if needs_aapt else ''
    xml = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"'
        f'{aapt_ns}\n'
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
