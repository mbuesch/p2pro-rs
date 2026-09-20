# InfiRay P2 Pro - Camera Hardware Interface Specification

## 1. Scope and Overview

The InfiRay P2 Pro is a USB-connected thermal imaging module.
It presents itself to the host as a standard UVC (USB Video Class) camera and simultaneously accepts proprietary vendor-specific commands over the USB control endpoint (endpoint 0).

The device exposes two independent functions that a driver must use together:

- **Video streaming function**: A UVC streaming interface delivering a composite frame that contains both a pseudo-coloured preview image and per-pixel radiometric data.
- **Vendor command function**: A mailbox-style command protocol transported over vendor-specific control transfers, used for configuration, device information queries and maintenance operations.

### 1.1 Terminology and Conventions

- All multi-byte *header fields* explicitly state their byte order. "LE" = little-endian, "BE" = big-endian.
- Hexadecimal values are written with a `0x` prefix.
- "OUT transfer" = host-to-device control transfer. "IN transfer" = device-to-host control transfer.
- A *mailbox address* is a 16-bit value carried in the `wIndex` field of the control transfer setup packet.
  Despite living in `wIndex`, it does **not** designate a USB interface number; it addresses an internal register block.
- `u8`, `u16`, `u32` denote unsigned integers of the given width.
- Frame coordinates use (width x height) ordering.

## 2. Device Identification and Enumeration

| Property            | Value                          |
| ------------------- | ------------------------------ |
| USB Vendor ID       | `0x0BDA`                       |
| USB Product ID      | `0x5830`                       |
| Device class        | UVC video camera ("USB Camera")|
| Command transport   | EP0 vendor control transfers   |

## 3. Video Streaming Interface

### 3.1 Stream Parameters

| Parameter      | Value                          |
| -------------- | ------------------------------ |
| Frame size     | 256 x 384 pixels               |
| Frame rate     | 25 fps                         |
| Pixel format   | YUY2 / YUYV 4:2:2 (2 bytes/px) |
| Bytes per frame| 196 608 (256 x 384 x 2)        |

### 3.2 Composite Frame Layout

Each frame is a vertically stacked pair of half-height images.
The sensor format is 256x192; the UVC frame packs **two** logical images of 256x192 into one 256x384 buffer:

```
byte offset 0         ┌──────────────────────────┐
                      │  Pseudo-colour image     │  256 x 192 px,
                      │  (YUY2 4:2:2 video)      │  2 bytes/px
byte offset 98 304    ├──────────────────────────┤
                      │  Radiometric image       │  256 x 192 px,
                      │  (u16 per pixel, LE)     │  2 bytes/px
byte offset 196 608   └──────────────────────────┘
```

- **Upper half:** A genuine YUY2 4:2:2 image, 256x192 pixels.
  Byte pairs follow Y-U-Y-V ordering.
  It is decoded to RGB with the standard BT.601 YUY2 conversion.
  The colour content is the thermal image rendered with the currently selected colour palette.
- **Lower half:** 256x192 samples of 16-bit little-endian unsigned radiometric data, one sample per sensor pixel.
  These values are proportional to scene radiance/temperature.

### 3.3 Decoding Procedure

1. Open the capture device (256x384 @ 25 fps, YUY2).
2. Disable automatic colour-space conversion on the capture path.
3. For each frame:
   a. Upper half Reshape to 192 rows x 256 columns x 2 bytes -> YUY2->RGB conversion -> preview image.
   b. Lower half: Reinterpret as 192 x 256 little-endian u16 -> radiometric image.

## 4. Vendor Command Transport

All configuration and query operations use vendor-specific control transfers on endpoint 0.
Two transfer templates exist:

### 4.1 Transfer Templates

**OUT:**

| Setup field     | Value  |
| --------------- | ------ |
| `bmRequestType` | `0x41` (host->device, vendor, interface) |
| `bRequest`      | `0x45` |
| `wValue`        | `0x0078` |
| `wIndex`        | mailbox address |
| Data            | command bytes |
   
**IN:**

| Setup field     | Value  |
| --------------- | ------ |
| `bmRequestType` | `0xC1` (device->host, vendor, interface) |
| `bRequest`      | `0x44` |
| `wValue`        | `0x0078` |
| `wIndex`        | mailbox address |
| Data            | response bytes |

### 4.2 Mailbox Address Map

The `wIndex` field selects an internal mailbox. The following addresses are used by the protocol:

| Address   | Role |
| --------- | ---- |
| `0x0200`  | **Status register** (IN, 1 byte). Polled to detect command completion. |
| `0x1D00`  | **Command header mailbox A** (OUT, 8 bytes). Used for standard writes without payload and for standard reads. |
| `0x9D00`  | **Command header mailbox B** (OUT, 8 bytes). Used for standard writes with payload and for long commands. |
| `0x1D08`  | **Data mailbox** (IN/OUT). Read data for standard reads; long-command parameter block (8 bytes); final short data segment of payload writes. |
| `0x9D08+n`| **Bulk data mailbox** (OUT). Bulk segments of payload writes, at offset `n`. |
| `0x1D10`  | **Long-read result mailbox** (IN). Result data of long commands. |

Note the pattern `0x9D00 = 0x1D00 | 0x8000` and `0x9D08 = 0x1D08 | 0x8000`:
Bit 15 of the mailbox address selects an alternate bank used when a payload stage follows the header stage.

### 4.3 Status Register and Ready Polling

Reading one byte from mailbox `0x0200` (IN template) yields the command
channel status:

| Bits    | Mask   | Meaning |
| ------- | ------ | ------- |
| 0       | `0x01` | Busy - command still executing |
| 1       | `0x02` | Busy - command still executing |
| 2 - 7   | `0xFC` | Error status - non-zero indicates an abnormal condition |

**Ready condition:** bits 0 and 1 are both clear.

**Wait until ready:**

1. Read the status byte.
2. If bits 0 and 1 are both clear -> the device is ready; stop polling.
3. Else, if any of bits 2-7 is set -> abort and report an error.
4. Else wait ~1 ms and repeat from step 1.

Consider a polling timeout.

A "wait until ready" must be performed after every header stage and after the final data stage of every command.
Intermediate bulk segments within one payload stage are written back-to-back without polling.

## 5. Command Transport Protocols

Three command exchange patterns exist:
**standard write**, **standard read**, and **long command** (which has write and read variants).
All share an 8-byte command header.

### 5.1 Common Command Header (8 bytes)

| Offset | Size | Field        | Byte order | Description |
| ------ | ---- | ------------ | ---------- | ----------- |
| 0      | 2    | Command code | LE         | 16-bit command code, including the direction flag |
| 2      | 4    | Parameter P  | see below  | 32-bit command parameter |
| 6      | 2    | Length L     | BE (u16)   | Payload length of the current 256-byte block; zero when unused |

**Parameter field byte order:**
For most commands the 4-byte parameter field is transmitted least-significant byte first.
For single-byte parameters only the byte at header offset 2 is significant.
**Exception:** for the SPI transfer command the address carried in this field is transmitted most-significant byte first.

When a command transports more than 256 bytes of payload, the payload is processed in blocks of at most 256 bytes; the parameter field is advanced by the payload offset of each block.

### 5.2 Standard Write

**Case A - No payload:**

1. OUT the 8-byte header to mailbox `0x1D00` (length field = 0).
2. Wait until ready.

**Case B - With payload:**

1. Split the payload into blocks of at most 256 bytes.
   For each block at payload offset `i`:
   1. OUT the 8-byte header to mailbox `0x9D00`, with the parameter field advanced by `i` and the length field set to the current block size.
   2. Wait until ready.
   3. Transmit the block in segments, walking an in-block offset `j` from 0 in steps of at most 64 bytes.
      Let `r = block_size - j` (bytes remaining in the block):
      - **r > 64:** OUT 64 bytes to mailbox `0x9D08 + j`. Do **not** poll; continue immediately with the next segment.
      - **8 < r <= 64:** OUT the first `r - 8` bytes to mailbox `0x9D08 + j`, then OUT the final 8 bytes to mailbox `0x1D08 + j + (r - 8)`.
      Wait until ready.
      - **r <= 8:** OUT the remaining `r` bytes to mailbox `0x1D08 + j`.
      Wait until ready.
   4. Continue with the next 256-byte block.

In short:
Each USB transfer carries at most 64 payload bytes; the bulk of a block goes to the `0x9D08`-based mailboxes, the final up-to-8 bytes of each block go to the `0x1D08`-based mailbox, and the device is polled once after the header and once after the last segment of each block.

### 5.3 Standard Read

1. If the requested length is 0, perform no transfer.
2. Otherwise, for each block of at most 256 bytes at payload offset `i` (block size `s = min(remaining, 256)`):
   a. OUT the 8-byte header to mailbox `0x1D00`, parameter field advanced by `i`, length field = `s`.
   b. Wait until ready.
   c. IN `s` bytes from mailbox `0x1D08` and append to the result buffer.
3. Concatenate.

### 5.4 Long Command

Long commands carry two extra 32-bit parameters (P3, P4) in a separate data stage.
The header uses mailboxes `0x9D00`/`0x1D08`.

**Long write:**

1. OUT the 8-byte header to mailbox `0x9D00`:
   - bytes 0-1: command code (LE)
   - bytes 2-3: P1 (BE u16)
   - bytes 4-7: P2 (BE u32)
2. OUT 8 bytes to mailbox `0x1D08`:
   - bytes 0-3: P3 (BE u32)
   - bytes 4-7: P4 (BE u32)
3. Wait until ready.

**Long read:**

1. OUT the header to mailbox `0x9D00` as above (P2 may be 0 when unused).
2. OUT 8 bytes to mailbox `0x1D08` with P3 = 0 and **P4 = requested read length** (both BE u32).
3. Wait until ready.
4. IN the requested number of bytes from mailbox `0x1D10`.

## 6. Command Set

### 6.1 Direction Flag

The 16-bit command code carries a direction bit:

| Direction            | Operation on code     |
| -------------------- | --------------------- |
| GET (read/query)     | base code (unchanged) |
| SET (write/configure)| base code OR `0x4000` |

Example:
The palette command base code `0x8409` is sent as `0x8409` for GET and as `0xC409` for SET.

### 6.2 Command Code Table

| Code     | Function               | Transport | Notes |
| -------- | ---------------------- | --------- | ----- |
| `0x0805` | System reset to ROM    | Standard write, no payload | Resets the device into ROM mode. Expect re-enumeration. |
| `0x8201` | SPI transfer           | Standard read/write with payload | Access to device memory/SPI flash; address parameter is sent most-significant byte first; payload may exceed 256 bytes using the block-offset mechanism. |
| `0x8405` | Get device information | Standard read | Parameter selects the information item. |
| `0x8409` | Pseudo-colour palette  | Standard read (1 byte) / standard write (1-byte payload) | Parameter selects the preview path (0 = default). Palette IDs. |
| `0x840C` | Shutter vtemp          | Standard read, 2 bytes | Shutter-related reference value. |
| `0x8514` | Temperature parameter (TPD) properties | Long read / long write | Per-parameter access. |
| `0x8B0D` | Current vtemp          | Standard read, 2 bytes | Current temperature-related raw value. |
| `0xC10F` | Preview start          | - | Start preview. |
| `0x020F` | Preview stop           | - | Stop preview. |
| `0x010A` | Y16 preview start      | - | Start Y16 preview. |
| `0x020A` | Y16 preview stop       | - | Stop Y16 preview. |

## 7. Command Details and Parameter Tables

### 7.1 Device Information Items (`0x8405`)

Issued as a standard read with the 4-byte parameter field set to the item index.
Each item has a fixed response length:

| Index | Item                    | Response length (bytes) |
| ----- | ----------------------- | ----------------------- |
| 0     | Chip ID                 | 8                       |
| 1     | Firmware compile date   | 8                       |
| 2     | Device qualification    | 8                       |
| 3     | IR (sensor) information | 26                      |
| 4     | Project information     | 4                       |
| 5     | Firmware build version  | 50                      |
| 6     | Part number (PN)        | 48                      |
| 7     | Serial number (SN)      | 16                      |
| 8     | Sensor ID               | 4                       |

Responses are returned as raw byte strings of the given length.
Textual items are expected to be fixed-width, NUL-padded strings.

### 7.2 Pseudo-Colour Palettes (`0x8409`)

* **SET:** Standard write, command code `0x8409 | 0x4000 = 0xC409`, parameter = preview path (0 = default), payload = one byte, the palette ID.
* **GET:** Standard read, command code `0x8409`, parameter = preview path, length = 1; returns the palette ID byte.

| ID  | Palette   |
| --- | --------- |
| 1   | White hot |
| 2   | Reserved  |
| 3   | Iron red  |
| 4   | Rainbow 1 |
| 5   | Rainbow 2 |
| 6   | Rainbow 3 |
| 7   | Red hot   |
| 8   | Hot red   |
| 9   | Rainbow 4 |
| 10  | Rainbow 5 |
| 11  | Black hot |

The palette affects only the pseudo-colour half-frame; The radiometric half-frame is unaffected.

### 7.3 Temperature Measurement Parameters - TPD (`0x8514`)

Accessed via the long-command transport.
P1 = parameter index, P2 = value (write) / 0 (read).
Reads return 2 bytes interpreted as a BE u16.

| P1  | Parameter                 | Unit / scale                   | Range     | Description |
| --- | ------------------------- | ------------------------------ | --------- | ------------- |
| 0   | Distance                  | 1/163.835 m per LSB (≈ 6.1 mm) | 0 - 32767 | Object distance used in the temperature computation |
| 1   | Reflected temperature     | 1 K per LSB                    | 0 - 1024  | Apparent reflected background temperature |
| 2   | Atmospheric temperature   | 1 K per LSB                    | 0 - 1024  | Temperature of the atmosphere between camera and object |
| 3   | Emissivity                | 1/127 per LSB (0.0 - 1.0)      | 0 - 127   | Object emissivity |
| 4   | Atmospheric transmittance | 1/127 per LSB (0.0 - 1.0)      | 0 - 127   | Atmospheric transmission coefficient |
| 5   | Gain select               | boolean                        | 0 - 1     | 0 = low gain, 1 = high gain (selects the measurement range) |

**Write procedure:**
Long write with code `0x8514 | 0x4000 = 0xC514`,
P1 = index, P2 = value, P3 = P4 = 0.

**Read procedure:**
Long read with code `0x8514`, P1 = index, P4 = 2; result = BE u16.

## 8. Operating Procedures

# 8.1 System-reset-to-ROM

The system-reset-to-ROM command (`0x0805`) is a maintenance operation, not part of normal shutdown; after issuing it the device must be treated as gone and re-enumerated.
