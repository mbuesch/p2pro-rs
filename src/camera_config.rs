//! P2Pro camera configuration protocol.

#![allow(dead_code)] //TODO

//TODO async!

use anyhow::{self as ah, Context as _, format_err as err};
use rusb::{Device, DeviceHandle, GlobalContext};
use std::{
    thread,
    time::{Duration, Instant},
};

// Control transfer templates.
const REQUEST_TYPE_OUT: u8 = 0x41;
const REQUEST_TYPE_IN: u8 = 0xC1;
const REQUEST_OUT: u8 = 0x45;
const REQUEST_IN: u8 = 0x44;
const REQUEST_VALUE: u16 = 0x0078;

// Mailbox addresses.
const MAILBOX_STATUS: u16 = 0x0200;
const MAILBOX_HEADER_A: u16 = 0x1D00;
const MAILBOX_HEADER_B: u16 = 0x9D00;
const MAILBOX_DATA: u16 = 0x1D08;
const MAILBOX_BULK: u16 = 0x9D08;
const MAILBOX_LONG_RESULT: u16 = 0x1D10;

// Status register bit masks.
const STATUS_BUSY: u8 = 0x03;
const STATUS_ERROR: u8 = 0xFC;

const HEADER_LEN: usize = 8;
/// Maximum payload processed per command header.
const BLOCK_LEN: usize = 256;
/// Maximum payload bytes per single USB transfer.
const SEGMENT_LEN: usize = 64;
/// Size of the tail segment routed through the data mailbox.
const SHORT_SEGMENT_LEN: usize = 8;

/// Timeout of a single USB control transfer.
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(1000);
/// Ready-polling timeout.
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// Interval between status polls while the device is busy.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Command code flag selecting the SET (write) direction.
const SET_FLAG: u16 = 0x4000;

// Command codes.
const CMD_RESET_TO_ROM: u16 = 0x0805;
const CMD_SPI_TRANSFER: u16 = 0x8201;
const CMD_DEVICE_INFO: u16 = 0x8405;
const CMD_PALETTE: u16 = 0x8409;
const CMD_SHUTTER_VTEMP: u16 = 0x840C;
const CMD_TPD: u16 = 0x8514;
const CMD_CURRENT_VTEMP: u16 = 0x8B0D;
const CMD_PREVIEW_START: u16 = 0xC10F;
const CMD_PREVIEW_STOP: u16 = 0x020F;
const CMD_Y16_PREVIEW_START: u16 = 0x010A;
const CMD_Y16_PREVIEW_STOP: u16 = 0x020A;

/// Default preview path selector for the palette commands.
const PREVIEW_PATH_DEFAULT: u32 = 0;

/// Pseudo-colour palettes for the preview half-frame.
///
/// The palette affects only the pseudo-colour half-frame;
/// the radiometric half-frame is unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Palette {
    WhiteHot = 1,
    IronRed = 3,
    Rainbow1 = 4,
    Rainbow2 = 5,
    Rainbow3 = 6,
    RedHot = 7,
    HotRed = 8,
    Rainbow4 = 9,
    Rainbow5 = 10,
    BlackHot = 11,
}

impl TryFrom<u8> for Palette {
    type Error = ah::Error;

    fn try_from(id: u8) -> ah::Result<Self> {
        match id {
            1 => Ok(Self::WhiteHot),
            3 => Ok(Self::IronRed),
            4 => Ok(Self::Rainbow1),
            5 => Ok(Self::Rainbow2),
            6 => Ok(Self::Rainbow3),
            7 => Ok(Self::RedHot),
            8 => Ok(Self::HotRed),
            9 => Ok(Self::Rainbow4),
            10 => Ok(Self::Rainbow5),
            11 => Ok(Self::BlackHot),
            other => Err(err!("Unknown palette ID: {other}")),
        }
    }
}

/// Device information item selector for [`CameraConfig::device_info`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInfoItem {
    ChipId,
    FirmwareCompileDate,
    DeviceQualification,
    IrSensorInfo,
    ProjectInfo,
    FirmwareBuildVersion,
    PartNumber,
    SerialNumber,
    SensorId,
}

impl DeviceInfoItem {
    /// Item index sent in the command parameter field.
    fn item_index(self) -> u32 {
        match self {
            Self::ChipId => 0,
            Self::FirmwareCompileDate => 1,
            Self::DeviceQualification => 2,
            Self::IrSensorInfo => 3,
            Self::ProjectInfo => 4,
            Self::FirmwareBuildVersion => 5,
            Self::PartNumber => 6,
            Self::SerialNumber => 7,
            Self::SensorId => 8,
        }
    }

    /// Fixed response length in bytes.
    fn response_len(self) -> usize {
        match self {
            Self::ChipId | Self::FirmwareCompileDate | Self::DeviceQualification => 8,
            Self::IrSensorInfo => 26,
            Self::ProjectInfo | Self::SensorId => 4,
            Self::FirmwareBuildVersion => 50,
            Self::PartNumber => 48,
            Self::SerialNumber => 16,
        }
    }
}

/// Temperature measurement parameter (TPD) selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum TpdParam {
    /// Object distance.
    Distance = 0,
    /// Apparent reflected background temperature.
    ReflectedTemperature = 1,
    /// Temperature of the intervening atmosphere.
    AtmosphericTemperature = 2,
    /// Object emissivity.
    Emissivity = 3,
    /// Atmospheric transmission coefficient.
    AtmosphericTransmittance = 4,
    /// Measurement range.
    GainSelect = 5,
}

/// Builds the 8-byte command header for standard commands.
fn command_header(code: u16, param: u32, param_be: bool, len: u16) -> [u8; HEADER_LEN] {
    let mut header = [0_u8; HEADER_LEN];
    header[0..2].copy_from_slice(&code.to_le_bytes());
    let param = if param_be {
        param.to_be_bytes()
    } else {
        param.to_le_bytes()
    };
    header[2..6].copy_from_slice(&param);
    header[6..8].copy_from_slice(&len.to_be_bytes());
    header
}

/// Builds the 8-byte header of a long command.
fn long_command_header(code: u16, p1: u16, p2: u32) -> [u8; HEADER_LEN] {
    let mut header = [0_u8; HEADER_LEN];
    header[0..2].copy_from_slice(&code.to_le_bytes());
    header[2..4].copy_from_slice(&p1.to_be_bytes());
    header[4..8].copy_from_slice(&p2.to_be_bytes());
    header
}

/// Builds the 8-byte extra parameter block of a long command.
fn long_command_params(p3: u32, p4: u32) -> [u8; 8] {
    let mut params = [0_u8; 8];
    params[0..4].copy_from_slice(&p3.to_be_bytes());
    params[4..8].copy_from_slice(&p4.to_be_bytes());
    params
}

/// P2Pro configuration channel.
///
/// Wraps an opened USB device handle and speaks the vendor command protocol.
pub struct CameraConfig {
    handle: DeviceHandle<GlobalContext>,
}

impl CameraConfig {
    /// Opens the given USB device for configuration commands.
    pub fn new(usb_device: &Device<GlobalContext>) -> ah::Result<Self> {
        let handle = usb_device.open().context("Failed to open USB device")?;
        Ok(Self { handle })
    }

    /// Performs a vendor control OUT transfer to the given mailbox.
    fn control_out(&self, mailbox: u16, data: &[u8]) -> ah::Result<()> {
        let written = self
            .handle
            .write_control(
                REQUEST_TYPE_OUT,
                REQUEST_OUT,
                REQUEST_VALUE,
                mailbox,
                data,
                TRANSFER_TIMEOUT,
            )
            .context("USB control OUT transfer failed")?;
        if written != data.len() {
            return Err(err!(
                "Short control OUT transfer to mailbox 0x{mailbox:04X}: {written} of {} bytes",
                data.len(),
            ));
        }
        Ok(())
    }

    /// Performs a vendor control IN transfer from the given mailbox.
    fn control_in(&self, mailbox: u16, buf: &mut [u8]) -> ah::Result<()> {
        let read = self
            .handle
            .read_control(
                REQUEST_TYPE_IN,
                REQUEST_IN,
                REQUEST_VALUE,
                mailbox,
                buf,
                TRANSFER_TIMEOUT,
            )
            .context("USB control IN transfer failed")?;
        if read != buf.len() {
            return Err(err!(
                "Short control IN transfer from mailbox 0x{mailbox:04X}: {read} of {} bytes",
                buf.len(),
            ));
        }
        Ok(())
    }

    /// Reads the command channel status register.
    fn status(&self) -> ah::Result<u8> {
        let mut buf = [0_u8; 1];
        self.control_in(MAILBOX_STATUS, &mut buf)?;
        Ok(buf[0])
    }

    /// Polls the status register until the device is ready.
    fn wait_ready(&self) -> ah::Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let status = self.status()?;
            if status & STATUS_BUSY == 0 {
                return Ok(());
            }
            if status & STATUS_ERROR != 0 {
                return Err(err!("Camera reported error status 0x{status:02X}"));
            }
            if Instant::now() >= deadline {
                return Err(err!("Timeout waiting for the camera to become ready"));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Standard write.
    fn standard_write(
        &self,
        code: u16,
        param: u32,
        param_be: bool,
        payload: &[u8],
    ) -> ah::Result<()> {
        if payload.is_empty() {
            // No payload, header via mailbox A only.
            self.control_out(MAILBOX_HEADER_A, &command_header(code, param, param_be, 0))?;
            self.wait_ready()
        } else {
            // Payload, processed in blocks of at most 256 bytes.
            for (index, block) in payload.chunks(BLOCK_LEN).enumerate() {
                let offset = index * BLOCK_LEN;
                let header = command_header(
                    code,
                    param.wrapping_add(offset as u32),
                    param_be,
                    block.len() as u16,
                );
                self.control_out(MAILBOX_HEADER_B, &header)?;
                self.wait_ready()?;
                self.write_block(block)?;
            }
            Ok(())
        }
    }

    /// Transmits one payload block in segments of at most 64 bytes,
    fn write_block(&self, block: &[u8]) -> ah::Result<()> {
        let mut offset = 0;
        while offset < block.len() {
            let remaining = block.len() - offset;
            if remaining > SEGMENT_LEN {
                // Bulk segment: written back-to-back without polling.
                self.control_out(
                    MAILBOX_BULK + offset as u16,
                    &block[offset..offset + SEGMENT_LEN],
                )?;
                offset += SEGMENT_LEN;
            } else if remaining > SHORT_SEGMENT_LEN {
                // Split tail: bulk mailbox first, last 8 bytes via data mailbox.
                let split = remaining - SHORT_SEGMENT_LEN;
                self.control_out(MAILBOX_BULK + offset as u16, &block[offset..offset + split])?;
                self.control_out(
                    MAILBOX_DATA + (offset + split) as u16,
                    &block[offset + split..],
                )?;
                self.wait_ready()?;
                offset += remaining;
            } else {
                // Short final segment via data mailbox.
                self.control_out(MAILBOX_DATA + offset as u16, &block[offset..])?;
                self.wait_ready()?;
                offset += remaining;
            }
        }
        Ok(())
    }

    /// Standard read.
    fn standard_read(
        &self,
        code: u16,
        param: u32,
        param_be: bool,
        len: usize,
    ) -> ah::Result<Vec<u8>> {
        let mut result = Vec::with_capacity(len);
        let mut offset = 0;
        while offset < len {
            let block_len = (len - offset).min(BLOCK_LEN);
            let header = command_header(
                code,
                param.wrapping_add(offset as u32),
                param_be,
                block_len as u16,
            );
            self.control_out(MAILBOX_HEADER_A, &header)?;
            self.wait_ready()?;
            let mut block = [0_u8; BLOCK_LEN];
            self.control_in(MAILBOX_DATA, &mut block)?;
            self.wait_ready()?;
            result.extend_from_slice(&block[..block_len]);
            offset += block_len;
        }
        Ok(result)
    }

    /// Standard-reads a 2-byte value and decodes it big-endian.
    fn standard_read_u16be(&self, code: u16) -> ah::Result<u16> {
        let data = self.standard_read(code, 0, false, 2)?;
        let bytes: [u8; 2] = data.as_slice().try_into()?;
        Ok(u16::from_be_bytes(bytes))
    }

    /// Long write.
    fn long_write(&self, code: u16, p1: u16, p2: u32, p3: u32, p4: u32) -> ah::Result<()> {
        self.control_out(MAILBOX_HEADER_B, &long_command_header(code, p1, p2))?;
        self.control_out(MAILBOX_DATA, &long_command_params(p3, p4))?;
        self.wait_ready()
    }

    /// Long read.
    fn long_read(&self, code: u16, p1: u16, p2: u32, len: usize) -> ah::Result<Vec<u8>> {
        self.control_out(MAILBOX_HEADER_B, &long_command_header(code, p1, p2))?;
        self.control_out(MAILBOX_DATA, &long_command_params(0, len as u32))?;
        self.wait_ready()?;
        let mut result = vec![0_u8; len];
        self.control_in(MAILBOX_LONG_RESULT, &mut result)?;
        self.wait_ready()?;
        Ok(result)
    }

    /// Reset the device to ROM settings.
    ///
    /// Maintenance operation only: the device re-enumerates afterwards and
    /// this handle must be discarded.
    pub fn reset_to_rom(self) -> ah::Result<()> {
        self.standard_write(CMD_RESET_TO_ROM, 0, false, &[])
    }

    /// Read a raw device information item.
    pub fn device_info(&self, item: DeviceInfoItem) -> ah::Result<Vec<u8>> {
        self.standard_read(
            CMD_DEVICE_INFO,
            item.item_index(),
            false,
            item.response_len(),
        )
    }

    /// Read a device information item as a string, trimming NUL padding.
    pub fn device_info_string(&self, item: DeviceInfoItem) -> ah::Result<String> {
        let raw = self.device_info(item)?;
        let end = raw.iter().position(|&byte| byte == 0).unwrap_or(raw.len());
        String::from_utf8(raw[..end].to_vec()).context("Device information is not valid UTF-8")
    }

    /// Read a summary of all available device information items.
    pub fn device_info_summary(&self) -> ah::Result<Vec<String>> {
        let items = [
            DeviceInfoItem::DeviceQualification,
            DeviceInfoItem::FirmwareBuildVersion,
            DeviceInfoItem::FirmwareCompileDate,
            DeviceInfoItem::PartNumber,
            DeviceInfoItem::SerialNumber,
        ];
        let mut summary = Vec::with_capacity(items.len());
        for item in items {
            if let Ok(value) = self.device_info(item) {
                let value = String::from_utf8(value).unwrap_or_default();
                summary.push(format!("{item:?}: {value}"));
            }
        }
        Ok(summary)
    }

    /// Select the pseudo-colour palette of the default preview path
    pub fn set_palette(&self, palette: Palette) -> ah::Result<()> {
        self.standard_write(
            CMD_PALETTE | SET_FLAG,
            PREVIEW_PATH_DEFAULT,
            false,
            &[palette as u8],
        )
    }

    /// Query the palette of the default preview path.
    pub fn palette(&self) -> ah::Result<Palette> {
        let data = self.standard_read(CMD_PALETTE, PREVIEW_PATH_DEFAULT, false, 1)?;
        let id = data.first().copied().context("Empty palette response")?;
        Palette::try_from(id)
    }

    /// Read the shutter-related reference value.
    pub fn shutter_vtemp(&self) -> ah::Result<u16> {
        self.standard_read_u16be(CMD_SHUTTER_VTEMP)
    }

    /// Read the current temperature-related raw value.
    pub fn current_vtemp(&self) -> ah::Result<u16> {
        self.standard_read_u16be(CMD_CURRENT_VTEMP)
    }

    /// Read a TPD parameter. Return the raw value.
    pub fn tpd_get(&self, param: TpdParam) -> ah::Result<u16> {
        let data = self.long_read(CMD_TPD, param as u16, 0, 2)?;
        let bytes: [u8; 2] = data.as_slice().try_into().context("Short TPD response")?;
        Ok(u16::from_be_bytes(bytes))
    }

    /// Write a raw TPD parameter value.
    ///
    /// The caller must keep the value within the parameter's range,
    /// see [`TpdParam`].
    pub fn tpd_set(&self, param: TpdParam, value: u16) -> ah::Result<()> {
        self.long_write(CMD_TPD | SET_FLAG, param as u16, u32::from(value), 0, 0)
    }

    /// Read the object emissivity (0.0 - 1.0).
    pub fn emissivity(&self) -> ah::Result<f32> {
        Ok(f32::from(self.tpd_get(TpdParam::Emissivity)?) / 127.0)
    }

    /// Set the object emissivity, clamped to 0.0 - 1.0.
    pub fn set_emissivity(&self, emissivity: f32) -> ah::Result<()> {
        let raw = (emissivity.clamp(0.0, 1.0) * 127.0).round() as u16;
        self.tpd_set(TpdParam::Emissivity, raw)
    }

    /// Read the object distance in metres used for temperature computation.
    pub fn distance(&self) -> ah::Result<f32> {
        Ok(f32::from(self.tpd_get(TpdParam::Distance)?) / 163.835)
    }

    /// Set the object distance in metres (0 - ~200 m).
    pub fn set_distance(&self, metres: f32) -> ah::Result<()> {
        if !(0.0..=200.0).contains(&metres) {
            return Err(err!("Distance out of range: {metres} m"));
        }
        let raw = (metres * 163.835).round() as u16;
        self.tpd_set(TpdParam::Distance, raw)
    }

    /// Read the atmospheric transmittance (0.0 - 1.0).
    pub fn atmospheric_transmittance(&self) -> ah::Result<f32> {
        //FIXME: I get 0x80. Should the divisor be 128 instead?
        Ok(f32::from(self.tpd_get(TpdParam::AtmosphericTransmittance)?) / 127.0)
    }

    /// Set the atmospheric transmittance, clamped to 0.0 - 1.0.
    pub fn set_atmospheric_transmittance(&self, transmittance: f32) -> ah::Result<()> {
        let raw = (transmittance.clamp(0.0, 1.0) * 127.0).round() as u16;
        self.tpd_set(TpdParam::AtmosphericTransmittance, raw)
    }

    /// Read the gain selection: false = low gain, true = high gain.
    pub fn high_gain(&self) -> ah::Result<bool> {
        Ok(self.tpd_get(TpdParam::GainSelect)? != 0)
    }

    /// Select the measurement range: false = low gain, true = high gain.
    pub fn set_high_gain(&self, high: bool) -> ah::Result<()> {
        self.tpd_set(TpdParam::GainSelect, high.into())
    }

    /// Write a payload to device memory / SPI flash.
    pub fn spi_write(&self, address: u32, payload: &[u8]) -> ah::Result<()> {
        self.standard_write(CMD_SPI_TRANSFER | SET_FLAG, address, true, payload)
    }

    /// Read device memory / SPI flash.
    pub fn spi_read(&self, address: u32, len: usize) -> ah::Result<Vec<u8>> {
        self.standard_read(CMD_SPI_TRANSFER, address, true, len)
    }

    /// Start the preview.
    pub fn preview_start(&self) -> ah::Result<()> {
        self.standard_write(CMD_PREVIEW_START, 0, false, &[])
    }

    /// Stop the preview.
    pub fn preview_stop(&self) -> ah::Result<()> {
        self.standard_write(CMD_PREVIEW_STOP, 0, false, &[])
    }

    /// Start the Y16 preview.
    pub fn y16_preview_start(&self) -> ah::Result<()> {
        self.standard_write(CMD_Y16_PREVIEW_START, 0, false, &[])
    }

    /// Stop the Y16 preview.
    pub fn y16_preview_stop(&self) -> ah::Result<()> {
        self.standard_write(CMD_Y16_PREVIEW_STOP, 0, false, &[])
    }

    /// Set the camera to its default configuration.
    pub fn set_default(&self) -> ah::Result<()> {
        self.set_emissivity(1.0)?;
        self.set_distance(0.2)?;
        self.set_high_gain(true)?;
        self.set_palette(Palette::WhiteHot)?;
        self.preview_stop()?;
        Ok(())
    }
}
