use alloy_primitives::{Address, FixedBytes, Keccak256, hex};
use eyre::{Result, WrapErr};
use indicatif::HumanDuration;
#[cfg(not(target_os = "macos"))]
use ocl::{Buffer, Context, Device, MemFlags, Platform, ProQue, Program, Queue};
#[cfg(not(target_os = "macos"))]
use rand::RngExt;
use std::fmt::Write;
use std::process::ExitCode;
use std::time::Duration;
#[cfg(not(target_os = "macos"))]
use std::time::Instant;

use crate::{AppConfig, Display};

#[cfg(target_os = "macos")]
pub(crate) mod metal;

static KERNEL_SRC: &str = include_str!("./kernels/keccak256.cl");

const CONTROL_CHARACTER: u8 = 0xff;
pub(super) const READBACK_INTERVAL_BATCHES: u32 = 8;

#[derive(Debug, Clone)]
pub struct MiningOutcome {
    pub salt: [u8; 32],
    pub address: Address,
    pub score: usize,
}

#[derive(Debug)]
pub struct MiningRun {
    pub outcome: Option<MiningOutcome>,
    pub runtime: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiningStop {
    FirstMatch,
    Timed {
        min_runtime: Option<Duration>,
        max_runtime: Option<Duration>,
    },
}

impl MiningStop {
    pub fn from_limits(min_runtime_secs: Option<u64>, max_runtime_secs: Option<u64>) -> Self {
        if min_runtime_secs.is_none() && max_runtime_secs.is_none() {
            Self::FirstMatch
        } else {
            Self::Timed {
                min_runtime: min_runtime_secs.map(Duration::from_secs),
                max_runtime: max_runtime_secs.map(Duration::from_secs),
            }
        }
    }

    fn initial_threshold(self, target: usize) -> usize {
        match self {
            Self::Timed {
                max_runtime: Some(_),
                ..
            } => 0,
            _ => target,
        }
    }

    /// Call at a mining batch boundary, after collecting any pending result.
    fn reached(self, elapsed: Duration, best_score: Option<usize>, target: usize) -> bool {
        let qualified = best_score.is_some_and(|score| score >= target);
        match self {
            Self::FirstMatch => qualified,
            Self::Timed {
                min_runtime,
                max_runtime,
            } => {
                max_runtime.is_some_and(|maximum| elapsed >= maximum)
                    || (min_runtime.is_none_or(|minimum| elapsed >= minimum) && qualified)
            }
        }
    }
}

pub(crate) fn mining_exit_code(score: Option<usize>, target: usize) -> ExitCode {
    if score.is_some_and(|score| score >= target) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

/// Given a `config` object with a factory address, a caller address, a keccak-256 hash
/// of the contract initialization code, search for salts using the native accelerator. The salts
/// enable the factory contract to deploy a contract to a gas-efficient address via CREATE2.
///
/// The 32-byte salt is constructed as follows:
///   - the 20-byte calling address (to prevent frontrunning)
///   - a random 4-byte segment (to prevent collisions with other runs)
///   - a 4-byte segment unique to each work group running in parallel
///   - a 4-byte nonce segment (incrementally stepped through during the run)
///
/// When a salt that will result in the creation of a gas-efficient contract
/// address is found, it will be displayed on the screen along with the resultant address
/// and the "score" (i.e. how many zero bytes) of the resultant address.
///
/// This method only searches for results better than what is already found. For example,
/// if a salt is found that results in an address with 3 zero bytes, the next salt
/// will only be displayed if it results in an address with at least 4 zero bytes anywhere.
///
/// This method is highly experimental and could certainly use further optimization.
/// Contributions are welcome as always!
pub fn start_miner(config: AppConfig, display: Option<Display>) -> Result<ExitCode> {
    if config.min_runtime_secs.is_some() || config.max_runtime_secs.is_some() {
        let abi = config.abi;
        let target = config.zeros;
        let stop = MiningStop::from_limits(config.min_runtime_secs, config.max_runtime_secs);
        let run = mine_once(config, stop)?;
        let exit_code = mining_exit_code(run.outcome.as_ref().map(|outcome| outcome.score), target);
        if let Some(outcome) = run.outcome {
            if abi {
                print_abi_encoded_result(&outcome.salt, outcome.address.as_slice(), outcome.score);
            } else {
                println!(
                    "0x{} => {} (Score: {}, Runtime: {})",
                    hex::encode(outcome.salt),
                    outcome.address,
                    outcome.score,
                    HumanDuration(run.runtime),
                );
            }
        } else if !abi {
            println!(
                "No candidate found (Runtime: {})",
                HumanDuration(run.runtime)
            );
        }
        return Ok(exit_code);
    }
    #[cfg(target_os = "macos")]
    {
        metal::start_miner(config, display).map(|()| ExitCode::SUCCESS)
    }
    #[cfg(not(target_os = "macos"))]
    {
        start_opencl_miner(config, display).map(|()| ExitCode::SUCCESS)
    }
}

#[cfg(not(target_os = "macos"))]
fn start_opencl_miner(config: AppConfig, mut display: Option<Display>) -> Result<()> {
    if !config.abi {
        println!("Preparing OpenCL Miner...",);
    }

    let start = Instant::now();

    let worksize = config.worksize;

    let mut found_list: Vec<String> = vec![];

    if let Some(display) = &display {
        display.start()?;
    }

    let platform = Platform::new(
        ocl::core::default_platform().wrap_err("failed to get default OpenCL platform")?,
    );
    let device =
        Device::by_idx_wrap(platform, 0_usize).wrap_err("failed to get default OpenCL device")?;
    let context = Context::builder()
        .platform(platform)
        .devices(device)
        .build()
        .wrap_err("failed to build OpenCL context")?;

    let program = Program::builder()
        .devices(device)
        .src(mk_kernel_src(&config))
        .build(&context)
        .wrap_err("failed to build OpenCL program")?;

    let queue = Queue::new(&context, device, None).wrap_err("failed to create OpenCL queue")?;
    let program_queue = ProQue::new(context, queue, program, Some(worksize));

    let mut rng = rand::rng();

    // set up variables for tracking performance
    let mut cumulative_nonce: u64 = 0;

    let mut previous_display_update = Instant::now();
    let mut pending_batches = 0_u32;

    let mut next_zeros: usize = config.zeros;

    let mut salt = FixedBytes::<4>::random();
    let mut nonce: [u32; 1] = rng.random();
    let mut solutions = vec![0_u64; 1];
    let solutions_buffer = Buffer::builder()
        .queue(program_queue.queue().clone())
        .flags(MemFlags::new().read_write())
        .len(1)
        .copy_host_slice(&solutions)
        .build()
        .wrap_err("failed to build solutions buffer")?;

    let kernel = program_queue
        .kernel_builder("hashMessage")
        .arg_named("message", u32::from_le_bytes(salt.0))
        .arg_named("nonce", nonce[0])
        .arg_named("min_zeros", next_zeros as u32)
        .arg_named("solutions", &solutions_buffer)
        .build()
        .wrap_err("failed to build OpenCL kernel")?;

    loop {
        salt = FixedBytes::<4>::random();
        kernel
            .set_arg("message", u32::from_le_bytes(salt.0))
            .wrap_err("failed to set message kernel arg")?;

        nonce = rng.random();
        kernel
            .set_arg("nonce", nonce[0])
            .wrap_err("failed to set nonce kernel arg")?;

        solutions[0] = 0;
        solutions_buffer
            .write(&solutions)
            .enq()
            .wrap_err("failed to reset solutions buffer")?;

        // repeatedly enqueue kernel to search for new addresses
        loop {
            // enqueue the kernel
            unsafe {
                kernel.enq().wrap_err("failed to enqueue OpenCL kernel")?;
            };

            if !config.abi && previous_display_update.elapsed().as_secs() >= 1 {
                previous_display_update = Instant::now();
                let attempts_per_sec =
                    f64::from(worksize) * cumulative_nonce as f64 / start.elapsed().as_secs_f64();

                if let Some(display) = &mut display {
                    display.update(attempts_per_sec, next_zeros, &found_list)?;
                }
            }

            // increment the cumulative nonce (does not reset after a match)
            cumulative_nonce += 1;
            pending_batches += 1;

            if pending_batches == READBACK_INTERVAL_BATCHES {
                solutions_buffer
                    .read(&mut solutions)
                    .enq()
                    .wrap_err("failed to read OpenCL solutions")?;
                pending_batches = 0;
            }

            // if at least one solution is found, end the loop
            if solutions[0] != 0 {
                break;
            }

            // if no solution has yet been found, increment the nonce
            nonce[0] += 1;
            kernel
                .set_arg("nonce", nonce[0])
                .wrap_err("failed to set nonce kernel arg")?;
        }

        // iterate over each solution, first converting to a fixed array
        for &solution in &solutions {
            if solution == 0 {
                continue;
            }

            let solution = solution.to_le_bytes();

            let mut solution_message = [0; 85];
            solution_message[0] = CONTROL_CHARACTER;
            solution_message[1..21].copy_from_slice(&config.factory);
            solution_message[21..41].copy_from_slice(&config.caller);
            solution_message[41..45].copy_from_slice(&salt[..]);
            solution_message[45..53].copy_from_slice(&solution);
            solution_message[53..].copy_from_slice(&config.codehash);

            // create new hash object
            let mut hash = Keccak256::new();

            // update with header
            hash.update(solution_message);

            // hash the payload and get the result
            let mut res: [u8; 32] = [0; 32];
            hash.finalize_into(&mut res);

            // get the address that results from the hash
            let address =
                <&Address>::try_from(&res[12..]).wrap_err("failed to derive address from hash")?;

            let zero_bytes = address.iter().filter(|byte| **byte == 0).count();

            if zero_bytes >= next_zeros {
                next_zeros = zero_bytes + 1;
                kernel
                    .set_arg("min_zeros", next_zeros as u32)
                    .wrap_err("failed to set min_zeros kernel arg")?;
            }

            let output = format!(
                "0x{}{}{} => {} (Score: {}, Runtime: {})",
                hex::encode(config.caller),
                hex::encode(salt),
                hex::encode(solution),
                address,
                zero_bytes,
                HumanDuration(start.elapsed()),
            );

            if config.abi {
                print_abi_encoded_result(&solution_message[21..53], address.as_slice(), zero_bytes);
                if config.one {
                    return Ok(());
                }
            }

            found_list.push(output);

            if config.one {
                return Ok(());
            }
        }
    }
}

pub fn benchmark_miner(config: AppConfig, warmup_batches: u64, batches: u64) -> Result<u128> {
    #[cfg(target_os = "macos")]
    {
        metal::benchmark_miner(&config, warmup_batches, batches)
    }
    #[cfg(not(target_os = "macos"))]
    {
        benchmark_opencl_miner(config, warmup_batches, batches)
    }
}

#[cfg(not(target_os = "macos"))]
fn benchmark_opencl_miner(config: AppConfig, warmup_batches: u64, batches: u64) -> Result<u128> {
    let worksize = config.worksize;
    let platform = Platform::new(
        ocl::core::default_platform().wrap_err("failed to get default OpenCL platform")?,
    );
    let device =
        Device::by_idx_wrap(platform, 0_usize).wrap_err("failed to get default OpenCL device")?;
    let context = Context::builder()
        .platform(platform)
        .devices(device)
        .build()
        .wrap_err("failed to build OpenCL context")?;
    let program = Program::builder()
        .devices(device)
        .src(mk_kernel_src(&config))
        .build(&context)
        .wrap_err("failed to build OpenCL program")?;
    let queue = Queue::new(&context, device, None).wrap_err("failed to create OpenCL queue")?;
    let program_queue = ProQue::new(context, queue, program, Some(worksize));

    let mut solutions = vec![0_u64; 1];
    let solutions_buffer = Buffer::builder()
        .queue(program_queue.queue().clone())
        .flags(MemFlags::new().write_only())
        .len(1)
        .copy_host_slice(&solutions)
        .build()
        .wrap_err("failed to build solutions buffer")?;
    let kernel = program_queue
        .kernel_builder("hashMessage")
        .arg_named("message", 0_u32)
        .arg_named("nonce", 0_u32)
        .arg_named(
            "min_zeros",
            u32::try_from(config.zeros).wrap_err("zero-byte target does not fit in u32")?,
        )
        .arg_named("solutions", &solutions_buffer)
        .build()
        .wrap_err("failed to build OpenCL kernel")?;

    for _ in 0..warmup_batches {
        unsafe {
            kernel.enq().wrap_err("failed to enqueue warmup kernel")?;
        }
    }
    program_queue
        .queue()
        .finish()
        .wrap_err("failed to finish warmup")?;
    solutions_buffer
        .write(&solutions)
        .enq()
        .wrap_err("failed to clear benchmark solutions")?;

    let start = Instant::now();
    for _ in 0..batches {
        unsafe {
            kernel
                .enq()
                .wrap_err("failed to enqueue benchmark kernel")?;
        }
    }
    program_queue
        .queue()
        .finish()
        .wrap_err("failed to finish benchmark")?;
    solutions_buffer
        .read(&mut solutions)
        .enq()
        .wrap_err("failed to read benchmark solutions")?;
    let elapsed_ns = start.elapsed().as_nanos();
    let attempts = u128::from(worksize) * u128::from(batches);
    Ok(attempts * 1_000_000_000 / elapsed_ns)
}

pub fn mine_once(config: AppConfig, stop: MiningStop) -> Result<MiningRun> {
    #[cfg(target_os = "macos")]
    {
        metal::mine_once(config, stop)
    }
    #[cfg(not(target_os = "macos"))]
    {
        mine_once_opencl(config, stop)
    }
}

#[cfg(not(target_os = "macos"))]
fn mine_once_opencl(config: AppConfig, stop: MiningStop) -> Result<MiningRun> {
    let worksize = config.worksize;

    let platform = Platform::new(
        ocl::core::default_platform().wrap_err("failed to get default OpenCL platform")?,
    );
    let device =
        Device::by_idx_wrap(platform, 0_usize).wrap_err("failed to get default OpenCL device")?;
    let context = Context::builder()
        .platform(platform)
        .devices(device)
        .build()
        .wrap_err("failed to build OpenCL context")?;

    let program = Program::builder()
        .devices(device)
        .src(mk_kernel_src(&config))
        .build(&context)
        .wrap_err("failed to build OpenCL program")?;

    let queue = Queue::new(&context, device, None).wrap_err("failed to create OpenCL queue")?;
    let program_queue = ProQue::new(context, queue, program, Some(worksize));

    let mut rng = rand::rng();
    let mut next_zeros = stop.initial_threshold(config.zeros);
    let mut best_outcome = None;

    let mut salt = FixedBytes::<4>::random();
    let mut nonce: [u32; 1] = rng.random();
    let mut solutions = vec![0_u64; 1];
    let solutions_buffer = Buffer::builder()
        .queue(program_queue.queue().clone())
        .flags(MemFlags::new().read_write())
        .len(1)
        .copy_host_slice(&solutions)
        .build()
        .wrap_err("failed to build solutions buffer")?;

    let kernel = program_queue
        .kernel_builder("hashMessage")
        .arg_named("message", u32::from_le_bytes(salt.0))
        .arg_named("nonce", nonce[0])
        .arg_named("min_zeros", next_zeros as u32)
        .arg_named("solutions", &solutions_buffer)
        .build()
        .wrap_err("failed to build OpenCL kernel")?;

    let start = Instant::now();
    loop {
        salt = FixedBytes::<4>::random();
        kernel
            .set_arg("message", u32::from_le_bytes(salt.0))
            .wrap_err("failed to set message kernel arg")?;

        nonce = rng.random();
        kernel
            .set_arg("nonce", nonce[0])
            .wrap_err("failed to set nonce kernel arg")?;

        solutions[0] = 0;
        solutions_buffer
            .write(&solutions)
            .enq()
            .wrap_err("failed to reset solutions buffer")?;

        let mut pending_batches = 0_u32;
        loop {
            unsafe {
                kernel.enq().wrap_err("failed to enqueue OpenCL kernel")?;
            };

            pending_batches += 1;

            if pending_batches == READBACK_INTERVAL_BATCHES {
                solutions_buffer
                    .read(&mut solutions)
                    .enq()
                    .wrap_err("failed to read OpenCL solutions")?;
                pending_batches = 0;
            }

            if solutions[0] != 0 {
                break;
            }

            if stop.reached(
                start.elapsed(),
                best_outcome.as_ref().map(|best: &MiningOutcome| best.score),
                config.zeros,
            ) {
                if pending_batches > 0 {
                    solutions_buffer
                        .read(&mut solutions)
                        .enq()
                        .wrap_err("failed to read OpenCL solutions")?;
                    if solutions[0] != 0 {
                        break;
                    }
                }
                return Ok(MiningRun {
                    outcome: best_outcome,
                    runtime: start.elapsed(),
                });
            }

            nonce[0] += 1;
            kernel
                .set_arg("nonce", nonce[0])
                .wrap_err("failed to set nonce kernel arg")?;
        }

        for &solution in &solutions {
            if solution == 0 {
                continue;
            }

            let outcome = mining_outcome(&config, &salt, solution)?;
            eyre::ensure!(
                outcome.score >= next_zeros,
                "OpenCL returned a solution below the requested score"
            );
            if best_outcome
                .as_ref()
                .is_none_or(|best| outcome.score > best.score)
            {
                next_zeros = outcome.score + 1;
                kernel
                    .set_arg("min_zeros", next_zeros as u32)
                    .wrap_err("failed to set min_zeros kernel arg")?;
                best_outcome = Some(outcome);
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
        }
    }
}

pub(super) fn mining_outcome(
    config: &AppConfig,
    salt: &FixedBytes<4>,
    solution: u64,
) -> Result<MiningOutcome> {
    let solution = solution.to_le_bytes();

    let mut solution_message = [0; 85];
    solution_message[0] = CONTROL_CHARACTER;
    solution_message[1..21].copy_from_slice(&config.factory);
    solution_message[21..41].copy_from_slice(&config.caller);
    solution_message[41..45].copy_from_slice(&salt[..]);
    solution_message[45..53].copy_from_slice(&solution);
    solution_message[53..].copy_from_slice(&config.codehash);

    let mut hash = Keccak256::new();
    hash.update(solution_message);

    let mut res: [u8; 32] = [0; 32];
    hash.finalize_into(&mut res);

    let address = <&Address>::try_from(&res[12..])
        .wrap_err("failed to derive address from hash")?
        .to_owned();
    let score = address.iter().filter(|byte| **byte == 0).count();

    let mut create2_salt = [0_u8; 32];
    create2_salt[0..20].copy_from_slice(&config.caller);
    create2_salt[20..24].copy_from_slice(&salt[..]);
    create2_salt[24..32].copy_from_slice(&solution);

    Ok(MiningOutcome {
        salt: create2_salt,
        address,
        score,
    })
}

pub(super) fn print_abi_encoded_result(salt: &[u8], address: &[u8], score: usize) {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(salt);
    encoded.extend_from_slice(&[0_u8; 12]);
    encoded.extend_from_slice(address);
    encoded.extend_from_slice(&[0_u8; 16]);
    encoded.extend_from_slice(&(score as u128).to_be_bytes());
    println!("0x{}", hex::encode(encoded));
}

pub(super) fn mk_kernel_defines(config: &AppConfig) -> String {
    let mut src = String::with_capacity(2048);

    let factory = config.factory.iter();
    let caller = config.caller.iter();
    let hash = config.codehash.iter();
    let hash = hash.enumerate().map(|(i, x)| (i + 52, x));

    for (i, x) in factory.chain(caller).enumerate().chain(hash) {
        let _ = writeln!(src, "#define S_{} {}u", i + 1, x);
    }

    src
}

#[cfg(not(target_os = "macos"))]
fn mk_kernel_src(config: &AppConfig) -> String {
    let mut src = mk_kernel_defines(config);

    src.push_str(KERNEL_SRC);

    src
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    #[ignore = "requires an OpenCL device"]
    fn opencl_scores_match_cpu_at_nonce_boundaries() -> Result<()> {
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
        let queue = ProQue::builder()
            .src(mk_kernel_src(&config))
            .dims(1_usize)
            .build()?;
        let solutions = Buffer::<u64>::builder()
            .queue(queue.queue().clone())
            .len(1)
            .build()?;
        let kernel = queue
            .kernel_builder("hashMessage")
            .arg_named("message", 0_u32)
            .arg_named("nonce", 0_u32)
            .arg_named("min_zeros", 0_u32)
            .arg(&solutions)
            .build()?;
        for tail in [0_u32, 0x1234_5678, u32::MAX] {
            let salt = FixedBytes::from(tail.to_le_bytes());
            let qualifying_nonce = (1_u64..100_000)
                .find(|&nonce| mining_outcome(&config, &salt, nonce).unwrap().score >= 2)
                .expect("fixed workload has a nonce with two zero bytes");
            // Exercise both words and the byte split used by packed state setup.
            for nonce in [
                0,
                1,
                0x00ff_ffff,
                0x0100_0000,
                u64::from(u32::MAX),
                1_u64 << 32,
                u64::MAX - 1,
                u64::MAX,
                qualifying_nonce,
            ] {
                let reference = mining_outcome(&config, &salt, nonce)?;
                kernel.set_arg("message", tail)?;
                kernel.set_arg("nonce", (nonce >> 32) as u32)?;
                for threshold in 0..=reference.score + 1 {
                    // A different initial word distinguishes no write from a
                    // matching zero nonce without changing the kernel contract.
                    let initial = nonce.wrapping_add(1);
                    solutions.write(&[initial][..]).enq()?;
                    kernel.set_arg("min_zeros", threshold as u32)?;
                    unsafe {
                        kernel
                            .cmd()
                            .global_work_offset(nonce as u32 as usize)
                            .enq()?;
                    }
                    let mut actual = [initial];
                    solutions.read(&mut actual[..]).enq()?;
                    let expected = if reference.score >= threshold {
                        nonce
                    } else {
                        initial
                    };
                    assert_eq!(
                        actual[0], expected,
                        "tail {tail}, nonce {nonce}, threshold {threshold}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn stop_requires_target_and_minimum_before_maximum() {
        let stop = MiningStop::from_limits(Some(10), Some(20));
        for score in [None, Some(0), Some(5)] {
            assert!(!stop.reached(Duration::from_secs(10), score, 6));
            assert!(!stop.reached(Duration::from_secs(19), score, 6));
        }
        for score in [Some(6), Some(7)] {
            assert!(!stop.reached(Duration::from_millis(9_999), score, 6));
            assert!(stop.reached(Duration::from_secs(10), score, 6));
        }
    }

    #[test]
    fn maximum_stops_with_fallback_or_no_candidate_and_overrides_minimum() {
        for minimum in [None, Some(10), Some(30)] {
            let stop = MiningStop::from_limits(minimum, Some(20));
            for score in [None, Some(0), Some(5), Some(6)] {
                assert!(stop.reached(Duration::from_secs(20), score, 6));
            }
            assert!(!stop.reached(Duration::from_millis(19_999), Some(5), 6));
        }
        assert!(MiningStop::from_limits(Some(30), Some(0)).reached(Duration::ZERO, None, 6));
    }

    #[test]
    fn absent_minimum_adds_no_delay_but_requires_qualification() {
        for stop in [
            MiningStop::FirstMatch,
            MiningStop::from_limits(None, Some(20)),
        ] {
            assert!(!stop.reached(Duration::ZERO, None, 6));
            assert!(!stop.reached(Duration::ZERO, Some(5), 6));
            assert!(stop.reached(Duration::ZERO, Some(6), 6));
        }
        let stop = MiningStop::from_limits(Some(10), None);
        assert!(!stop.reached(Duration::from_secs(1_000), None, 6));
        assert!(!stop.reached(Duration::from_secs(1_000), Some(5), 6));
        assert!(stop.reached(Duration::from_secs(10), Some(6), 6));
    }

    #[test]
    fn stop_mode_owns_fallback_threshold() {
        assert_eq!(MiningStop::FirstMatch.initial_threshold(6), 6);
        assert_eq!(
            MiningStop::from_limits(Some(10), None).initial_threshold(6),
            6
        );
        assert_eq!(
            MiningStop::from_limits(None, Some(20)).initial_threshold(6),
            0
        );
        assert_eq!(
            MiningStop::from_limits(Some(30), Some(20)).initial_threshold(6),
            0
        );
    }

    #[test]
    fn mining_exit_status_requires_target() {
        for score in [None, Some(0), Some(5)] {
            assert_eq!(mining_exit_code(score, 6), ExitCode::from(2));
        }
        assert_eq!(mining_exit_code(Some(6), 6), ExitCode::SUCCESS);
        assert_eq!(mining_exit_code(Some(7), 6), ExitCode::SUCCESS);
    }
}
