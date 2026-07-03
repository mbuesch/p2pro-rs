# P2Pro Thermal Viewer

A minimal, from-scratch Rust rewrite of the Python `p2prolive_app.py` viewer.
It does **not** try to replicate every feature of the Python app - just the
core live thermal display:

- Live false-color ("ironbow"-style) view of the Infiray P2Pro thermal camera
- A temperature color-scale legend next to the image
- Markers for the current frame's coldest and hottest pixels, with their
  temperature labels
- Automatic scaling: the color range always stretches to the current frame's
  min/max temperature (like the Python app's "autoscale" mode - there is no
  manual range mode here, to keep things simple)

This app has been developed with major use of AI agent assistance.

## How it talks to the camera

The P2Pro shows up as a standard UVC webcam. This app opens it directly via
Video4Linux2 (the `v4l` crate) and requests raw `YUYV` frames at 256x384.
Just like the existing `p2pro.py`, the top half of that buffer is a normal
8-bit preview (ignored here) and the bottom half is actually raw 16-bit
temperature samples packed into what looks like YUYV bytes. See
`src/camera.rs` for the exact decode, which mirrors `p2pro.py`'s
`raw()`/`temperature()` methods (`raw/64 - 273.2` -> degrees Celsius).

## Running

```sh
cargo run --release
```

By default it opens `/dev/video4`. Pass a different device path as the first
argument if needed:

```sh
cargo run --release -- /dev/video2
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
