//! Command line tool for the InfiRay P2Pro thermal camera.

#![forbid(unsafe_code)]

use anyhow::{self as ah, Context as _, format_err as err};
use clap::{Parser, Subcommand, ValueEnum};
use p2pro_hw::{CameraConfig, PRODUCT_ID, Palette, VENDOR_ID};
use std::{io::Read as _, path::PathBuf, process::exit};
use tokio::task::spawn_blocking;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// USB bus number of the device (default: first P2Pro found).
    #[arg(long, requires = "address")]
    bus: Option<u8>,

    /// USB device address of the device (default: first P2Pro found).
    #[arg(long, requires = "bus")]
    address: Option<u8>,

    /// Allow potentially destructive operations.
    #[arg(long)]
    i_know_what_i_am_doing: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a device information summary.
    Info,
    /// Reset the device to factory settings. The device re-enumerates afterwards.
    Reset,
    /// Read from SPI flash.
    SpiRead {
        /// Start address (decimal or 0x-prefixed hex).
        #[arg(value_parser = parse_u32)]
        address: u32,
        /// Number of bytes to read (decimal or 0x-prefixed hex).
        #[arg(value_parser = parse_u32)]
        length: u32,
        /// Write the data to this file instead of a hex dump on stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Write to SPI flash.
    SpiWrite {
        /// Start address (decimal or 0x-prefixed hex).
        #[arg(value_parser = parse_u32)]
        address: u32,
        /// File with the data to write ('-' for stdin).
        input: PathBuf,
    },
    /// Enable or disable the preview.
    Preview {
        #[arg(value_enum)]
        state: OnOff,
    },
    /// Set the preview palette. Without argument, print the current palette.
    Palette {
        #[arg(value_enum)]
        palette: Option<PaletteArg>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum PaletteArg {
    WhiteHot,
    IronRed,
    Rainbow1,
    Rainbow2,
    Rainbow3,
    RedHot,
    HotRed,
    Rainbow4,
    Rainbow5,
    BlackHot,
}

impl From<PaletteArg> for Palette {
    fn from(p: PaletteArg) -> Self {
        match p {
            PaletteArg::WhiteHot => Palette::WhiteHot,
            PaletteArg::IronRed => Palette::IronRed,
            PaletteArg::Rainbow1 => Palette::Rainbow1,
            PaletteArg::Rainbow2 => Palette::Rainbow2,
            PaletteArg::Rainbow3 => Palette::Rainbow3,
            PaletteArg::RedHot => Palette::RedHot,
            PaletteArg::HotRed => Palette::HotRed,
            PaletteArg::Rainbow4 => Palette::Rainbow4,
            PaletteArg::Rainbow5 => Palette::Rainbow5,
            PaletteArg::BlackHot => Palette::BlackHot,
        }
    }
}

fn parse_u32(s: &str) -> Result<u32, String> {
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse(),
    };
    r.map_err(|e| e.to_string())
}

async fn open_device(bus: Option<u8>, address: Option<u8>) -> ah::Result<nusb::Device> {
    let info = nusb::list_devices()
        .await
        .context("Failed to list USB devices")?
        .find(|d| match (bus, address) {
            (Some(bus), Some(address)) => d.busnum() == bus && d.device_address() == address,
            _ => d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID,
        })
        .ok_or_else(|| err!("P2Pro device not found."))?;
    info.open().await.context("Failed to open USB device")
}

fn hex_dump(start: u32, data: &[u8]) {
    for (i, line) in data.chunks(16).enumerate() {
        let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        println!("{:08x}: {}", start as usize + i * 16, hex.join(" "));
    }
}

async fn run(args: &Args) -> ah::Result<()> {
    let device = open_device(args.bus, args.address).await?;
    let mut conf = CameraConfig::from_hw_access(device);

    match &args.command {
        Command::Info => {
            for line in conf.device_info_summary().await? {
                println!("{line}");
            }
        }
        Command::Reset => {
            conf.reset_to_rom().await?;
            println!("Device reset to factory settings.");
        }
        Command::SpiRead {
            address,
            length,
            output,
        } => {
            let data = conf.spi_read(*address, *length as usize).await?;
            match output {
                Some(path) => tokio::fs::write(&path, &data)
                    .await
                    .with_context(|| format!("Failed to write {}", path.display()))?,
                None => hex_dump(*address, &data),
            }
        }
        Command::SpiWrite { address, input } => {
            if !args.i_know_what_i_am_doing {
                return Err(err!(
                    "You are trying to write to the SPI flash. \
                    This is disallowed by default. \
                    Use --i-know-what-i-am-doing to proceed."
                ));
            }
            let data = if input.as_os_str() == "-" {
                spawn_blocking(move || -> ah::Result<Vec<u8>> {
                    let mut buf = vec![];
                    std::io::stdin().read_to_end(&mut buf)?;
                    Ok(buf)
                })
                .await
                .context("Failed to spawn blocking task")?
                .context("Failed to read from stdin")?
            } else {
                tokio::fs::read(&input)
                    .await
                    .with_context(|| format!("Failed to read {}", input.display()))?
            };
            conf.spi_write(*address, &data).await?;
        }
        Command::Preview { state } => match state {
            OnOff::On => conf.preview_start().await?,
            OnOff::Off => conf.preview_stop().await?,
        },
        Command::Palette { palette } => match palette {
            Some(p) => conf.set_palette((*p).into()).await?,
            None => println!("{:?}", conf.palette().await?),
        },
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();
    if let Err(e) = run(&args).await {
        eprintln!("ERROR: {e:?}");
        exit(1);
    }
}
