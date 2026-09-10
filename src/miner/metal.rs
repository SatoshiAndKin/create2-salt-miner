use std::ffi::c_void;
use std::mem::size_of;
use std::time::{Duration, Instant};

use alloy_primitives::{FixedBytes, hex};
use eyre::{Context, ContextCompat, OptionExt, Result, ensure, eyre};
use indicatif::HumanDuration;
use metal::{
    Buffer, CommandQueue, CompileOptions, ComputePipelineState, Device, MTLCommandBufferStatus,
    MTLResourceOptions, MTLSize, NSUInteger,
};
use objc::rc::autoreleasepool;
use rand::RngExt;

use super::{
    KERNEL_SRC, MiningOutcome, MiningRun, MiningStop, READBACK_INTERVAL_BATCHES, mining_outcome,
    mk_kernel_defines, print_abi_encoded_result,
};
use crate::{AppConfig, Display};

const SOLUTION_WORDS: usize = 3;

struct MetalMiner {
    queue: CommandQueue,
    pipeline: ComputePipelineState,
    solutions: Buffer,
    worksize: u32,
    threads_per_group: NSUInteger,
}

impl MetalMiner {
    fn new(config: &AppConfig) -> Result<Self> {
        ensure!(config.worksize > 0, "worksize must be greater than zero");
        let device = Device::system_default()
            .or_else(|| Device::all().into_iter().next())
            .ok_or_eyre("no Metal device is available")?;
        autoreleasepool(|| {
            let options = CompileOptions::new();
            options.set_fast_math_enabled(true);
            let library = device
                .new_library_with_source(&metal_kernel_src(config), &options)
                .map_err(|error| eyre!("failed to build Metal library: {error}"))?;
            let function = library
                .get_function("hashMessage", None)
                .map_err(|error| eyre!("failed to load Metal kernel: {error}"))?;
            let pipeline = device
                .new_compute_pipeline_state_with_function(&function)
                .map_err(|error| eyre!("failed to build Metal compute pipeline: {error}"))?;
            let execution_width = pipeline.thread_execution_width();
            let maximum_threads = pipeline.max_total_threads_per_threadgroup();
            ensure!(
                execution_width > 0 && maximum_threads >= execution_width,
                "Metal reported an invalid compute thread width"
            );
            let preferred_threads = maximum_threads.min(256);
            let threads_per_group = (preferred_threads / execution_width).max(1) * execution_width;
            let solutions = device.new_buffer(
                u64::try_from(SOLUTION_WORDS * size_of::<u32>())?,
                MTLResourceOptions::StorageModeShared,
            );
            ensure!(
                !solutions.contents().is_null(),
                "Metal solution buffer has no CPU-visible storage"
            );
            Ok(Self {
                queue: device.new_command_queue(),
                pipeline,
                solutions,
                worksize: config.worksize,
                threads_per_group,
            })
        })
    }

    fn run_batches(
        &self,
        salt_tail: u32,
        first_nonce_hi: u32,
        min_zeros: u32,
        batches: u32,
    ) -> Result<Option<u64>> {
        ensure!(batches > 0, "Metal batch count must be greater than zero");
        unsafe {
            self.solutions
                .contents()
                .cast::<u32>()
                .write_bytes(0, SOLUTION_WORDS);
        }
        autoreleasepool(|| {
            let command_buffer = self.queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&self.pipeline);
            encoder.set_buffer(3, Some(&self.solutions), 0);
            let threads_per_grid = MTLSize {
                width: NSUInteger::from(self.worksize),
                height: 1,
                depth: 1,
            };
            let threads_per_group = MTLSize {
                width: self.threads_per_group,
                height: 1,
                depth: 1,
            };
            for batch in 0..batches {
                let nonce_hi = first_nonce_hi.wrapping_add(batch);
                encoder.set_bytes(
                    0,
                    size_of::<u32>() as NSUInteger,
                    std::ptr::from_ref(&salt_tail).cast::<c_void>(),
                );
                encoder.set_bytes(
                    1,
                    size_of::<u32>() as NSUInteger,
                    std::ptr::from_ref(&nonce_hi).cast::<c_void>(),
                );
                encoder.set_bytes(
                    2,
                    size_of::<u32>() as NSUInteger,
                    std::ptr::from_ref(&min_zeros).cast::<c_void>(),
                );
                encoder.dispatch_threads(threads_per_grid, threads_per_group);
            }
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();
            ensure!(
                command_buffer.status() == MTLCommandBufferStatus::Completed,
                "Metal command buffer failed with status {:?}",
                command_buffer.status()
            );
            let words = unsafe {
                std::slice::from_raw_parts(self.solutions.contents().cast::<u32>(), SOLUTION_WORDS)
            };
            Ok((words[0] != 0).then(|| (u64::from(words[2]) << 32) | u64::from(words[1])))
        })
    }
}

pub(crate) fn list_devices() -> Result<()> {
    let devices = Device::all();
    ensure!(!devices.is_empty(), "no Metal devices are available");
    println!("Metal Devices:");
    for (index, device) in devices.iter().enumerate() {
        println!("\tDevice ID: {index}");
        println!("\tName: {}", device.name());
        println!("\tLow power: {}", device.is_low_power());
        println!("\tHeadless: {}", device.is_headless());
        println!("\tUnified memory: {}", device.has_unified_memory());
    }
    let selected = Device::system_default().ok_or_eyre("no default Metal device is available")?;
    println!("Selected Device: {}", selected.name());
    Ok(())
}

pub(super) fn start_miner(config: AppConfig, mut display: Option<Display>) -> Result<()> {
    if !config.abi {
        println!("Preparing Metal Miner...");
    }
    let engine = MetalMiner::new(&config)?;
    let start = Instant::now();
    let mut found_list = Vec::new();
    let mut previous_display_update = Instant::now();
    let mut completed_batches = 0_u64;
    let mut next_zeros = config.zeros;
    let mut rng = rand::rng();
    if let Some(display) = &display {
        display.start()?;
    }

    loop {
        let salt = FixedBytes::<4>::random();
        let salt_tail = u32::from_le_bytes(salt.0);
        let mut nonce_hi: u32 = rng.random();
        loop {
            let solution = engine.run_batches(
                salt_tail,
                nonce_hi,
                u32::try_from(next_zeros).context("zero-byte target does not fit in u32")?,
                READBACK_INTERVAL_BATCHES,
            )?;
            completed_batches = completed_batches
                .checked_add(u64::from(READBACK_INTERVAL_BATCHES))
                .context("completed Metal batch count overflow")?;
            if !config.abi && previous_display_update.elapsed() >= Duration::from_secs(1) {
                previous_display_update = Instant::now();
                let attempts_per_sec = f64::from(config.worksize) * completed_batches as f64
                    / start.elapsed().as_secs_f64();
                if let Some(display) = &mut display {
                    display.update(attempts_per_sec, next_zeros, &found_list)?;
                }
            }
            nonce_hi = nonce_hi.wrapping_add(READBACK_INTERVAL_BATCHES);
            let Some(solution) = solution else {
                continue;
            };
            let outcome = mining_outcome(&config, &salt, solution)?;
            ensure!(
                outcome.score >= next_zeros,
                "Metal returned a solution below the requested score"
            );
            next_zeros = outcome.score + 1;
            if config.abi {
                print_abi_encoded_result(&outcome.salt, outcome.address.as_slice(), outcome.score);
                if config.one {
                    return Ok(());
                }
            }
            found_list.push(format!(
                "0x{} => {} (Score: {}, Runtime: {})",
                hex::encode(outcome.salt),
                outcome.address,
                outcome.score,
                HumanDuration(start.elapsed()),
            ));
            if config.one {
                return Ok(());
            }
            break;
        }
    }
}

pub(super) fn benchmark_miner(
    config: &AppConfig,
    warmup_batches: u64,
    batches: u64,
) -> Result<u128> {
    ensure!(
        batches > 0,
        "benchmark batch count must be greater than zero"
    );
    let engine = MetalMiner::new(config)?;
    if warmup_batches > 0 {
        engine.run_batches(
            0,
            0,
            21,
            u32::try_from(warmup_batches).context("warmup batch count does not fit in u32")?,
        )?;
    }
    let start = Instant::now();
    engine.run_batches(
        0,
        u32::try_from(warmup_batches).context("warmup batch count does not fit in u32")?,
        21,
        u32::try_from(batches).context("benchmark batch count does not fit in u32")?,
    )?;
    let elapsed_ns = start.elapsed().as_nanos();
    ensure!(elapsed_ns > 0, "Metal benchmark duration is zero");
    let attempts = u128::from(config.worksize)
        .checked_mul(u128::from(batches))
        .context("Metal benchmark attempt count overflow")?;
    Ok(attempts * 1_000_000_000 / elapsed_ns)
}

pub(super) fn mine_once(config: AppConfig, stop: MiningStop) -> Result<MiningRun> {
    let engine = MetalMiner::new(&config)?;
    let start = Instant::now();
    let mut next_zeros = stop.initial_threshold(config.zeros);
    let mut best_outcome = None;
    let mut rng = rand::rng();

    loop {
        let salt = FixedBytes::<4>::random();
        let salt_tail = u32::from_le_bytes(salt.0);
        let mut nonce_hi: u32 = rng.random();
        loop {
            let solution = engine.run_batches(
                salt_tail,
                nonce_hi,
                u32::try_from(next_zeros).context("zero-byte target does not fit in u32")?,
                READBACK_INTERVAL_BATCHES,
            )?;
            nonce_hi = nonce_hi.wrapping_add(READBACK_INTERVAL_BATCHES);
            if let Some(solution) = solution {
                let outcome = mining_outcome(&config, &salt, solution)?;
                ensure!(
                    outcome.score >= next_zeros,
                    "Metal returned a solution below the requested score"
                );
                if best_outcome
                    .as_ref()
                    .is_none_or(|best: &MiningOutcome| outcome.score > best.score)
                {
                    next_zeros = outcome.score + 1;
                    best_outcome = Some(outcome);
                }
            }
            if stop.reached(
                start.elapsed(),
                best_outcome.as_ref().map(|best| best.score),
                config.zeros,
            ) {
                return Ok(MiningRun {
                    outcome: best_outcome,
                    runtime: start.elapsed(),
                });
            }
            if solution.is_some() {
                break;
            }
        }
    }
}

fn metal_kernel_src(config: &AppConfig) -> String {
    let mut source = String::from("#define METAL_BACKEND 1\n");
    source.push_str(&mk_kernel_defines(config));
    source.push_str(KERNEL_SRC);
    source
}

#[cfg(test)]
mod tests {
    use alloy_primitives::FixedBytes;
    use eyre::Result;
    use metal::Device;

    use super::MetalMiner;
    use crate::{AppConfig, miner::mining_outcome};

    #[test]
    #[ignore = "requires a Metal device"]
    fn metal_scores_match_cpu_at_nonce_boundaries() -> Result<()> {
        let config = AppConfig {
            factory: [0x11; 20],
            caller: [0x22; 20],
            codehash: [0x33; 32],
            worksize: 1,
            zeros: 0,
            one: true,
            abi: true,
            min_runtime_secs: None,
            max_runtime_secs: None,
        };
        let engine = MetalMiner::new(&config)?;
        eprintln!(
            "Metal execution width {}, maximum threads {}, selected threads {}",
            engine.pipeline.thread_execution_width(),
            engine.pipeline.max_total_threads_per_threadgroup(),
            engine.threads_per_group
        );
        for tail in [0_u32, 0x1234_5678, u32::MAX] {
            let salt = FixedBytes::from(tail.to_le_bytes());
            let qualifying_nonce = (0_u32..10_000)
                .find(|&nonce| {
                    mining_outcome(&config, &salt, u64::from(nonce) << 32)
                        .unwrap()
                        .score
                        >= 1
                })
                .expect("fixed workload has a positive-score nonce");
            for nonce in [0, 1, u32::MAX - 1, u32::MAX, qualifying_nonce] {
                let solution = u64::from(nonce) << 32;
                let reference = mining_outcome(&config, &salt, solution)?;
                for threshold in 0..=reference.score + 1 {
                    let actual = engine.run_batches(tail, nonce, threshold as u32, 1)?;
                    assert_eq!(
                        actual,
                        (reference.score >= threshold).then_some(solution),
                        "salt tail {tail}, nonce {nonce}, threshold {threshold}"
                    );
                }
            }
            let solution = engine.run_batches(tail, u32::MAX, 0, 2)?.unwrap();
            assert!(solution == (u64::from(u32::MAX) << 32) || solution == 0);
        }
        Ok(())
    }

    #[test]
    fn metal_kernel_returns_a_cpu_verified_nonce() -> Result<()> {
        if Device::system_default().is_none() && Device::all().is_empty() {
            return Ok(());
        }

        let config = AppConfig {
            factory: [0x11; 20],
            caller: [0x22; 20],
            codehash: [0x33; 32],
            worksize: 256,
            zeros: 0,
            one: true,
            abi: false,
            min_runtime_secs: None,
            max_runtime_secs: None,
        };
        let salt_tail = 0x1234_5678;
        let nonce_hi = 0x9abc_def0;
        let engine = MetalMiner::new(&config)?;
        let solution = engine
            .run_batches(salt_tail, nonce_hi, 0, 1)?
            .expect("a zero-byte target accepts every candidate");

        assert_eq!(solution >> 32, u64::from(nonce_hi));
        assert!((solution as u32) < config.worksize);
        let salt = FixedBytes::from(salt_tail.to_le_bytes());
        let outcome = mining_outcome(&config, &salt, solution)?;
        assert_eq!(&outcome.salt[20..24], &salt_tail.to_le_bytes());
        assert_eq!(&outcome.salt[24..32], &solution.to_le_bytes());

        Ok(())
    }
}
