#!/bin/sh
# Regenerate all icon assets from assets/icon.svg.
# Requires: rsvg-convert (librsvg), convert (ImageMagick), python3 + python3-lxml
set -e

basedir="$(realpath "$0" | xargs dirname)"
cd "$basedir/.."

command -v rsvg-convert >/dev/null 2>&1 || { echo "rsvg-convert not found. Please install librsvg." >&2; exit 1; }
command -v convert >/dev/null 2>&1 || { echo "convert not found. Please install ImageMagick." >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not found. Please install python3." >&2; exit 1; }
python3 -c "import lxml" 2>/dev/null || { echo "python3-lxml not found. Please install python3-lxml." >&2; exit 1; }

# PNG icons
for s in 32 48 64 128 256 512; do
    rsvg-convert -w $s -h $s assets/icon.svg -o assets/icon-${s}x${s}.png
done

# Android mipmap WebP (legacy raster icons)
for size_dir in "48:mipmap-mdpi" "72:mipmap-hdpi" "96:mipmap-xhdpi" "144:mipmap-xxhdpi" "192:mipmap-xxxhdpi"; do
    size="${size_dir%%:*}"
    dir="${size_dir##*:}"
    mkdir -p "android/res/${dir}"
    rsvg-convert -w "$size" -h "$size" assets/icon.svg -o /tmp/ic_launcher_${size}.png
    convert /tmp/ic_launcher_${size}.png -define webp:lossless=true android/res/${dir}/ic_launcher.webp
done

# Android adaptive icon — background (plain white fill)
mkdir -p android/res/drawable
cat > android/res/drawable/ic_launcher_background.xml << 'EOF'
<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="108dp"
    android:height="108dp"
    android:viewportWidth="108"
    android:viewportHeight="108">
    <path
        android:fillColor="#FFFFFF"
        android:pathData="M0,0h108v108h-108z" />
</vector>
EOF

mkdir -p android/res/mipmap-anydpi-v26
cat > android/res/mipmap-anydpi-v26/ic_launcher.xml << 'EOF'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@drawable/ic_launcher_background"/>
    <foreground android:drawable="@drawable/ic_launcher_foreground"/>
</adaptive-icon>
EOF

# Android adaptive icon — foreground (SVG → Android Vector Drawable)
mkdir -p android/res/drawable-v24
python3 assets/svg2vd.py assets/icon.svg android/res/drawable-v24/ic_launcher_foreground.xml
