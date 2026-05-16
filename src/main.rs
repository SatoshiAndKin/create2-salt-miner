use alloy_primitives::hex;
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result, WrapErr, eyre};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

mod display;
mod gpgpu;
mod miner;

pub use display::Display;
pub use miner::start_miner;

#[derive(Parser, Debug, Serialize, Deserialize)]
struct MineArgs {
    /// Factory Address
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    factory: Option<String>,

    /// Caller Address
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    caller: Option<String>,

    /// Initcode Hash
    #[arg(short = 'i', long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    codehash: Option<String>,

    /// Work Size
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    worksize: Option<u32>,

    /// Minimum zeros to look for
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    zeros: Option<usize>,

    /// Exit after the first matching salt
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    once: bool,

    /// Print the first matching salt as abi.encode(bytes32,address,uint256)
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    abi: bool,
}

#[derive(Parser, Debug, Serialize, Deserialize)]
struct BenchArgs {
    /// Factory Address
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    factory: Option<String>,

    /// Caller Address
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    caller: Option<String>,

    /// Initcode Hash
    #[arg(short = 'i', long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    codehash: Option<String>,

    /// Work Size
    #[arg(short, long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    worksize: Option<u32>,

    /// Timed kernel batches
    #[arg(long, default_value_t = 20)]
    batches: u64,

    /// Untimed warmup kernel batches
    #[arg(long, default_value_t = 3)]
    warmup_batches: u64,
}

#[derive(Subcommand, Debug, Serialize, Deserialize)]
enum Commands {
    /// Start Create2 Salt Miner
    Mine(MineArgs),
    /// Benchmark OpenCL mining throughput
    Bench(BenchArgs),
    /// List available OpenCL Platforms (& Devices), including default
    List {},
}

#[derive(Parser, Debug, Serialize, Deserialize)]
#[command(name = "Salty", author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    mode: Commands,
}

#[derive(Debug)]
pub struct AppConfig {
    pub factory: [u8; 20],
    pub caller: [u8; 20],
    pub codehash: [u8; 32],
    pub worksize: u32,
    pub zeros: usize,
    pub once: bool,
    pub abi: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.mode {
        Commands::Mine(args) => {
            let unwrapped: MineArgs = Figment::new()
                .merge(Toml::file("salty.toml"))
                .merge(Serialized::defaults(args))
                .extract()
                .wrap_err("failed to load configuration")?;

            if !unwrapped.abi {
                println!("{:#?}", unwrapped);
            }

            let app_config = AppConfig {
                factory: decode_fixed(
                    &unwrapped
                        .factory
                        .unwrap_or_else(|| "0x0000000000FFe8B47B3e2130213B802212439497".to_owned()),
                    "factory",
                )?,
                caller: decode_fixed(
                    &unwrapped
                        .caller
                        .ok_or_eyre("missing required caller address")?,
                    "caller",
                )?,
                codehash: decode_fixed(
                    &unwrapped
                        .codehash
                        .ok_or_eyre("missing required initcode hash")?,
                    "codehash",
                )?,
                worksize: unwrapped.worksize.unwrap_or(0x4400000_u32),
                zeros: unwrapped.zeros.unwrap_or(6_usize),
                once: unwrapped.once,
                abi: unwrapped.abi,
            };

            let display = if app_config.abi {
                None
            } else {
                Some(Display::new()?)
            };

            start_miner(app_config, display)?;
        }
        Commands::List {} => {
            gpgpu::list_devices()?;
        }
        Commands::Bench(args) => {
            let unwrapped: BenchArgs = Figment::new()
                .merge(Toml::file("salty.toml"))
                .merge(Serialized::defaults(args))
                .extract()
                .wrap_err("failed to load configuration")?;

            let app_config = AppConfig {
                factory: decode_fixed(
                    &unwrapped
                        .factory
                        .unwrap_or_else(|| "0x0000000000FFe8B47B3e2130213B802212439497".to_owned()),
                    "factory",
                )?,
                caller: decode_fixed(
                    &unwrapped
                        .caller
                        .ok_or_eyre("missing required caller address")?,
                    "caller",
                )?,
                codehash: decode_fixed(
                    &unwrapped
                        .codehash
                        .ok_or_eyre("missing required initcode hash")?,
                    "codehash",
                )?,
                worksize: unwrapped.worksize.unwrap_or(0x4400000_u32),
                zeros: 21,
                once: false,
                abi: true,
            };

            let attempts_per_sec =
                miner::benchmark_miner(app_config, unwrapped.warmup_batches, unwrapped.batches)?;
            println!("METRIC attempts_per_sec={attempts_per_sec}");
        }
    }

    Ok(())
}

fn decode_fixed<const N: usize>(value: &str, field: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(value).wrap_err_with(|| format!("invalid {field} hex"))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| eyre!("{field} must be {N} bytes, got {}", bytes.len()))
}
