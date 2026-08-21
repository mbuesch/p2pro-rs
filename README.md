# InfiRay P2Pro Thermal Camera Viewer

A minimal InfiRay P2Pro thermal camera viewer.

Features:

- Live false-color ("ironbow"-style) view of the InfiRay P2Pro thermal camera.
- A temperature color-scale legend next to the image.
- Markers for the current frame's coldest and hottest pixels, with their temperature labels.
- Automatic scaling: the color range always stretches to the current frame's min/max temperature.
- Saving of the thermal image to a PNG file.

## Operating System Support

- **Linux**
- **Android**

## How it talks to the camera

The P2Pro shows up as a standard UVC webcam and requests raw `YUYV` frames at 256x384.
The top half of that buffer is a normal 8-bit preview (ignored here) and the bottom half is actually raw 16-bit temperature samples packed into what looks like YUYV bytes.

How that YUYV stream is obtained depends on the platform:

- **Linux desktop**: opened directly via Video4Linux2.
- **Android**: Android does not expose a V4L2. Instead, the app drives the P2Pro's USB Video Class protocol itself over `libusb` (the `rusb` crate).

## Running on Linux

First install [Rust](https://www.rust-lang.org/tools/install) and then build the app with cargo:

```sh
cargo build --release
```

Then run the built executable:

```sh
./target/release/p2pro-rs
```

The app will probe `/dev/video*` for a P2Pro camera and open the first one it finds.

If you want to specify a particular device, you can pass it as the first argument:

```sh
./target/release/p2pro-rs /dev/video2
```

There is no need to install the app.
You can just copy the `p2pro-rs` binary to a convenient location and run it from there.

## Running on Android

First install [Rust](https://www.rust-lang.org/tools/install) on the build PC (Linux).

Before running the Android build script, ensure you have the Android NDK and SDK installed and properly configured on the build PC.
The easiest way to get them is to install [Android Studio](https://developer.android.com/studio), which includes both.
For the build script to work, you need to set the some environment variables to point to your Android NDK and SDK installations.

```sh
# Set this to the path of your Android SDK installation.
export ANDROID_HOME="$HOME/Android/Sdk"

# Set this to the path of your Android NDK installation.
# Adjust the VERSION part to match the installed NDK version.
export ANDROID_NDK_HOME="$HOME/Android/Sdk/ndk/VERSION"

# Add Android SDK platform-tools to PATH for ADB access.
export PATH="$HOME/Android/Sdk/platform-tools/:$PATH"
```

On the build PC (Linux) run the provided script to build the Android packages:

```sh
./android-build.sh
```

Install the generated APK on your Android device (via ADB).
Plug in your Android device, ensure Developer Mode, USB debugging and Sideloading are enabled, and run:

```sh
./android-install.sh
```

## License

This app has been developed with use of AI agent assistance and with manual software development methods.

Copyright (c) 2026 Michael Büsch

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

## Acknowledgements

This program has initially been AI-derived from the p2pro-live Python application.

Copyright of the original p2pro-live application:

Copyright (c) 2024 Klaus Schwarzburg
