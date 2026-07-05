# InfiRay P2Pro Thermal Camera Viewer

A minimal InfiRay P2Pro thermal camera viewer.

Features:

- Live false-color ("ironbow"-style) view of the InfiRay P2Pro thermal camera
- A temperature color-scale legend next to the image
- Markers for the current frame's coldest and hottest pixels, with their
  temperature labels
- Automatic scaling: the color range always stretches to the current frame's
  min/max temperature.

## How it talks to the camera

The P2Pro shows up as a standard UVC webcam.
This app opens it directly via Video4Linux2 (the `v4l` crate) and requests raw `YUYV` frames at 256x384.
The top half of that buffer is a normal 8-bit preview (ignored here) and the bottom half is actually raw 16-bit temperature samples packed into what looks like YUYV bytes.

## Running

You can build and run it directly from this source tree with the Rust build system `cargo`:

```sh
cargo run --release -- /dev/video2
```

## License

This app has been developed with use of AI agent assistance and with manual software development methods.

Copyright (c) 2026 Michael Büsch

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Acknowledgements

This program has initially been AI-derived from the p2pro-live Python application.

Copyright of the original p2pro-live application:

Copyright (c) 2024 Klaus Schwarzburg
