use alloy_primitives::hex;
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result, WrapErr, eyre};
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod display;
mod gpgpu;
mod miner;
mod server;

pub use display::Display;
pub use miner::start_miner;

pub const DEFAULT_FACTORY: &str = "0x0000000000FFe8B47B3e2130213B802212439497";
const DEFAULT_BENCH_CALLER: &str = "0x0000000000000000000000000000000000000000";
const DEFAULT_BENCH_CODEHASH: &str =
    "0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107";

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

    /// Only output the best matching salt (runs until min-runtime-secs has passed and target is met)
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    one: bool,

    /// Mine for at least this many seconds, then return the best qualifying result found
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    min_runtime_secs: Option<u64>,

    /// Maximum runtime in seconds. If exceeded, returns the best result found so far, even if it doesn't meet the target zeros.
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    max_runtime_secs: Option<u64>,

    /// Print the first matching salt as abi.encode(bytes32,address,uint256)
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    abi: bool,

    /// Remote HTTP mining server base URL
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    remote_server: Option<String>,
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

#[derive(Parser, Debug, Serialize, Deserialize)]
struct ServeArgs {
    /// Host to bind the HTTP server to
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    host: Option<String>,

    /// Port to bind the HTTP server to
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    port: Option<u16>,

    /// SQLite cache file path
    #[arg(long)]
    #[serde(skip_serializing_if = "::std::option::Option::is_none")]
    cache_path: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Serialize, Deserialize)]
enum Commands {
    /// Start Create2 Salt Miner
    Mine(MineArgs),
    /// Benchmark accelerator mining throughput
    Bench(BenchArgs),
    /// Start remote HTTP mining server
    Serve(ServeArgs),
    /// List available accelerator devices and the selected device
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
    pub one: bool,
    pub abi: bool,
    pub min_runtime_secs: Option<u64>,
    pub max_runtime_secs: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.mode {
        Commands::Mine(args) => {
            let unwrapped: MineArgs = Figment::new()
                .merge(Toml::file("salty.toml"))
                .merge(Serialized::defaults(args))
                .extract()
                .wrap_err("failed to load configuration")?;

            if matches!(unwrapped.min_runtime_secs, Some(0)) {
                return Err(eyre!("min_runtime_secs must be greater than zero"));
            }

            if !unwrapped.abi {
                println!("{:#?}", unwrapped);
            }

            let factory = unwrapped
                .factory
                .unwrap_or_else(|| DEFAULT_FACTORY.to_owned());
            let caller = unwrapped
                .caller
                .ok_or_eyre("missing required caller address")?;
            let codehash = unwrapped
                .codehash
                .ok_or_eyre("missing required initcode hash")?;
            let worksize = unwrapped.worksize.unwrap_or(0x4400000_u32);
            let zeros = unwrapped.zeros.unwrap_or(6_usize);

            if let Some(remote_server) = unwrapped.remote_server {
                let response = server::mine_remote(
                    &remote_server,
                    server::MineRequest {
                        factory: Some(factory),
                        caller,
                        codehash,
                        worksize: Some(worksize),
                        zeros: Some(zeros),
                        min_runtime_secs: unwrapped.min_runtime_secs,
                    },
                )
                .await?;
                print_remote_mine_response(response, unwrapped.abi)?;
                return Ok(());
            }

            let app_config = AppConfig {
                factory: decode_fixed(&factory, "factory")?,
                caller: decode_fixed(&caller, "caller")?,
                codehash: decode_fixed(&codehash, "codehash")?,
                worksize,
                zeros,
                one: unwrapped.one,
                abi: unwrapped.abi,
                min_runtime_secs: unwrapped.min_runtime_secs,
                max_runtime_secs: unwrapped.max_runtime_secs,
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

            let app_config = build_bench_app_config(&unwrapped)?;

            let attempts_per_sec =
                miner::benchmark_miner(app_config, unwrapped.warmup_batches, unwrapped.batches)?;
            println!("METRIC attempts_per_sec={attempts_per_sec}");
        }
        Commands::Serve(args) => {
            let unwrapped: ServeArgs = Figment::new()
                .merge(Toml::file("salty.toml"))
                .merge(Serialized::defaults(args))
                .extract()
                .wrap_err("failed to load configuration")?;
            server::start_server(server::ServerConfig {
                host: unwrapped.host.unwrap_or_else(|| "0.0.0.0".to_owned()),
                port: unwrapped.port.unwrap_or(3000),
                cache_path: unwrapped
                    .cache_path
                    .unwrap_or_else(|| PathBuf::from("salty-cache.sqlite")),
            })
            .await?;
        }
    }

    Ok(())
}

fn build_bench_app_config(args: &BenchArgs) -> Result<AppConfig> {
    Ok(AppConfig {
        factory: decode_fixed(
            args.factory.as_deref().unwrap_or(DEFAULT_FACTORY),
            "factory",
        )?,
        caller: decode_fixed(
            args.caller.as_deref().unwrap_or(DEFAULT_BENCH_CALLER),
            "caller",
        )?,
        codehash: decode_fixed(
            args.codehash.as_deref().unwrap_or(DEFAULT_BENCH_CODEHASH),
            "codehash",
        )?,
        worksize: args.worksize.unwrap_or(0x4400000_u32),
        zeros: 21,
        one: false,
        abi: true,
        min_runtime_secs: None,
        max_runtime_secs: None,
    })
}

pub fn decode_fixed<const N: usize>(value: &str, field: &str) -> Result<[u8; N]> {
    let bytes = hex::decode(value).wrap_err_with(|| format!("invalid {field} hex"))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| eyre!("{field} must be {N} bytes, got {}", bytes.len()))
}

fn print_remote_mine_response(response: server::MineResponse, abi: bool) -> Result<()> {
    if abi {
        let salt = decode_fixed::<32>(
            response
                .salt
                .as_deref()
                .ok_or_eyre("remote server did not return a salt")?,
            "salt",
        )?;
        let address = decode_fixed::<20>(
            response
                .address
                .as_deref()
                .ok_or_eyre("remote server did not return an address")?,
            "address",
        )?;
        let score = response
            .score
            .ok_or_eyre("remote server did not return a score")?;
        miner::print_abi_encoded_result(&salt, &address, score);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&response)
                .wrap_err("failed to serialize remote mining response")?
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_config_has_built_in_defaults() -> Result<()> {
        let args = BenchArgs {
            factory: None,
            caller: None,
            codehash: None,
            worksize: None,
            batches: 20,
            warmup_batches: 3,
        };

        let config = build_bench_app_config(&args)?;

        assert_eq!(config.factory, decode_fixed(DEFAULT_FACTORY, "factory")?);
        assert_eq!(config.caller, [0_u8; 20]);
        assert_eq!(
            config.codehash,
            decode_fixed(DEFAULT_BENCH_CODEHASH, "codehash")?
        );
        assert_eq!(config.worksize, 0x4400000_u32);
        assert_eq!(config.zeros, 21);
        assert!(!config.one);
        assert!(config.abi);

        Ok(())
    }

    #[test]
    fn bench_config_honors_explicit_args() -> Result<()> {
        let args = BenchArgs {
            factory: Some("0x1111111111111111111111111111111111111111".to_owned()),
            caller: Some("0x2222222222222222222222222222222222222222".to_owned()),
            codehash: Some(
                "0x3333333333333333333333333333333333333333333333333333333333333333".to_owned(),
            ),
            worksize: Some(128),
            batches: 20,
            warmup_batches: 3,
        };

        let config = build_bench_app_config(&args)?;

        assert_eq!(config.factory, [0x11_u8; 20]);
        assert_eq!(config.caller, [0x22_u8; 20]);
        assert_eq!(config.codehash, [0x33_u8; 32]);
        assert_eq!(config.worksize, 128);

        Ok(())
    }
}
