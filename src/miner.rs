use alloy_primitives::{Address, FixedBytes, Keccak256, hex};
use eyre::{Result, WrapErr};
use indicatif::HumanDuration;
use ocl::{Buffer, Context, Device, MemFlags, Platform, ProQue, Program, Queue};
use rand::RngExt;
use std::fmt::Write;
use std::time::Instant;

use crate::{AppConfig, Display};

static KERNEL_SRC: &str = include_str!("./kernels/keccak256.cl");

const CONTROL_CHARACTER: u8 = 0xff;

/// Given a `config` object with a factory address, a caller address, a keccak-256 hash
/// of the contract initialization code, search for salts using OpenCL that will enable
/// the factory contract to deploy a contract to a gas-efficient address via CREATE2.
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
/// will only be displayed if it results in an address with 4 leading zero bytes.
///
/// This method is highly experimental and could certainly use further optimization.
/// Contributions are welcome as always!
pub fn start_miner(config: AppConfig, mut display: Option<Display>) -> Result<()> {
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
    let mut previous_display_nonce: u64 = 0;

    let mut next_zeros: usize = config.zeros;

    loop {
        // construct the 4-byte message to hash, leaving last 8 of salt empty
        let salt = FixedBytes::<4>::random();
        let salt_buffer = Buffer::builder()
            .queue(program_queue.queue().clone())
            .flags(MemFlags::new().read_only())
            .len(4)
            .copy_host_slice(&salt[..])
            .build()
            .wrap_err("failed to build salt buffer")?;

        // for more uniformly distributed nonces, we shall initialize it to a random value
        let mut nonce: [u32; 1] = rng.random();

        let mut solutions: Vec<u64> = vec![0; 1];
        let solutions_buffer = Buffer::builder()
            .queue(program_queue.queue().clone())
            .flags(MemFlags::new().write_only())
            .len(1)
            .copy_host_slice(&solutions)
            .build()
            .wrap_err("failed to build solutions buffer")?;

        let kernel = program_queue
            .kernel_builder("hashMessage")
            .arg_named("message", Some(&salt_buffer))
            .arg_named("nonce", nonce[0])
            .arg_named("min_zeros", next_zeros as u32)
            .arg_named("solutions", &solutions_buffer)
            .build()
            .wrap_err("failed to build OpenCL kernel")?;

        // repeatedly enqueue kernel to search for new addresses
        loop {
            // enqueue the kernel
            unsafe {
                kernel.enq().wrap_err("failed to enqueue OpenCL kernel")?;
            };
            program_queue
                .queue()
                .flush()
                .wrap_err("failed to flush OpenCL queue")?;

            if !config.abi && previous_display_update.elapsed().as_secs() >= 1 {
                let display_elapsed = previous_display_update.elapsed().as_secs_f64();
                let completed_batches = cumulative_nonce - previous_display_nonce;
                previous_display_update = Instant::now();
                previous_display_nonce = cumulative_nonce;
                let attempts_per_sec =
                    f64::from(worksize) * completed_batches as f64 / display_elapsed;

                if let Some(display) = &mut display {
                    display.update(attempts_per_sec, next_zeros, &found_list)?;
                }
            }

            // increment the cumulative nonce (does not reset after a match)
            cumulative_nonce += 1;

            // read the solutions from the device, blocking until the kernel completes
            solutions_buffer
                .read(&mut solutions)
                .enq()
                .wrap_err("failed to read OpenCL solutions")?;

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

            let output = format!(
                "0x{}{}{} => {} (Score: {}, Runtime: {})",
                hex::encode(config.caller),
                hex::encode(salt),
                hex::encode(solution),
                address,
                zero_bytes,
                HumanDuration(start.elapsed()),
            );

            if zero_bytes >= next_zeros {
                next_zeros = zero_bytes + 1;
            }

            if config.abi {
                print_abi_encoded_result(&solution_message[21..53], address, zero_bytes);
                if config.once {
                    return Ok(());
                }
            }

            found_list.push(output);

            if config.once {
                return Ok(());
            }
        }
    }
}

pub fn benchmark_miner(config: AppConfig, warmup_batches: u64, batches: u64) -> Result<u128> {
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

    let salt = FixedBytes::<4>::ZERO;
    let salt_buffer = Buffer::builder()
        .queue(program_queue.queue().clone())
        .flags(MemFlags::new().read_only())
        .len(4)
        .copy_host_slice(&salt[..])
        .build()
        .wrap_err("failed to build salt buffer")?;
    let solutions = vec![0_u64; 1];
    let solutions_buffer = Buffer::builder()
        .queue(program_queue.queue().clone())
        .flags(MemFlags::new().write_only())
        .len(1)
        .copy_host_slice(&solutions)
        .build()
        .wrap_err("failed to build solutions buffer")?;
    let kernel = program_queue
        .kernel_builder("hashMessage")
        .arg_named("message", Some(&salt_buffer))
        .arg_named("nonce", 0_u32)
        .arg_named("min_zeros", 21_u32)
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
    let elapsed_ns = start.elapsed().as_nanos();
    let attempts = u128::from(worksize) * u128::from(batches);
    Ok(attempts * 1_000_000_000 / elapsed_ns)
}

fn print_abi_encoded_result(salt: &[u8], address: &Address, score: usize) {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(salt);
    encoded.extend_from_slice(&[0_u8; 12]);
    encoded.extend_from_slice(address.as_slice());
    encoded.extend_from_slice(&[0_u8; 16]);
    encoded.extend_from_slice(&(score as u128).to_be_bytes());
    println!("0x{}", hex::encode(encoded));
}

fn mk_kernel_src(config: &AppConfig) -> String {
    let mut src = String::with_capacity(2048 + KERNEL_SRC.len());

    let factory = config.factory.iter();
    let caller = config.caller.iter();
    let hash = config.codehash.iter();
    let hash = hash.enumerate().map(|(i, x)| (i + 52, x));

    for (i, x) in factory.chain(caller).enumerate().chain(hash) {
        let _ = writeln!(src, "#define S_{} {}u", i + 1, x);
    }

    src.push_str(KERNEL_SRC);

    src
}
