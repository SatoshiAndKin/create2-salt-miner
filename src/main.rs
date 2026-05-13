use alloy_primitives::hex;
use clap::{Parser, Subcommand};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::process;

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
}

#[derive(Subcommand, Debug, Serialize, Deserialize)]
enum Commands {
    /// Start Create2 Salt Miner
    Mine(MineArgs),
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
}

fn main() {
    let cli = Cli::parse();

    match &cli.mode {
        Commands::Mine(args) => {
            let unwrapped: MineArgs = Figment::new()
                .merge(Toml::file("salty.toml"))
                .merge(Serialized::defaults(args))
                .extract()
                .unwrap();

            println!("{:#?}", unwrapped);

            if unwrapped.caller.is_none() || unwrapped.codehash.is_none() {
                eprintln!("Insufficient arguments provided. Please see --help for usage.");
                process::exit(1);
            }

            let app_config = AppConfig {
                factory: hex::decode(
                    unwrapped
                        .factory
                        .unwrap_or("0x0000000000FFe8B47B3e2130213B802212439497".to_string()),
                )
                .unwrap()
                .try_into()
                .unwrap(),
                caller: hex::decode(unwrapped.caller.unwrap_or("0x00".to_string()))
                    .unwrap()
                    .try_into()
                    .unwrap(),
                codehash: hex::decode(unwrapped.codehash.unwrap_or("0x00".to_string()))
                    .unwrap()
                    .try_into()
                    .unwrap(),
                worksize: unwrapped.worksize.unwrap_or(0x4400000_u32),
                zeros: unwrapped.zeros.unwrap_or(1_usize),
            };

            let display = Display::new();

            start_miner(app_config, display);
        }
        Commands::List {} => {
            gpgpu::list_devices();
        }
    }
}
