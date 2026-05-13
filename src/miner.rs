use alloy_primitives::{hex, Address, FixedBytes, Keccak256};
use ocl::{Buffer, Context, Device, MemFlags, Platform, ProQue, Program, Queue};
use rand::RngExt;
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

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
/// and the "score" (i.e. how many leading zero bytes) of the resultant address.
///
/// This method only searches for results better than what is already found. For example,
/// if a salt is found that results in an address with 3 leading zero bytes, the next salt
/// will only be displayed if it results in an address with 4 leading zero bytes.
///
/// This method is highly experimental and could certainly use further optimization.
/// Contributions are welcome as always!
pub fn start_miner(config: AppConfig, display: Display) {
    println!("Preparing OpenCL Miner...",);

    let worksize = config.worksize;
    let workfactor = (worksize as u128) / 1_000_000;

    let mut found_list: Vec<String> = vec![];

    display.start();

    let platform = Platform::new(ocl::core::default_platform().unwrap());
    let device = Device::by_idx_wrap(platform, 0_usize).unwrap();
    let context = Context::builder()
        .platform(platform)
        .devices(device)
        .build()
        .unwrap();

    let program = Program::builder()
        .devices(device)
        .src(mk_kernel_src(&config))
        .build(&context)
        .unwrap();

    let queue = Queue::new(&context, device, None).unwrap();
    let program_queue = ProQue::new(context, queue, program, Some(worksize));

    let mut rng = rand::rng();

    // set up variables for tracking performance
    let mut cumulative_nonce: u64 = 0;

    // the previous timestamp of printing to the terminal
    let mut previous_time: u64 = 0;

    // the last work duration in milliseconds
    let mut work_duration_millis: u64 = 0;

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
            .unwrap();

        // reset nonce & create a buffer to view it in little-endian
        // for more uniformly distributed nonces, we shall initialize it to a random value
        let mut nonce: [u32; 1] = rng.random();

        let mut nonce_buffer = Buffer::builder()
            .queue(program_queue.queue().clone())
            .flags(MemFlags::new().read_only())
            .len(1)
            .copy_host_slice(&nonce)
            .build()
            .unwrap();

        let mut solutions: Vec<u64> = vec![0; 1];
        let solutions_buffer = Buffer::builder()
            .queue(program_queue.queue().clone())
            .flags(MemFlags::new().write_only())
            .len(1)
            .copy_host_slice(&solutions)
            .build()
            .unwrap();

        // repeatedly enqueue kernel to search for new addresses
        loop {
            // build the kernel and define the type of each buffer
            let kernel = program_queue
                .kernel_builder("hashMessage")
                .arg_named("message", None::<&Buffer<u8>>)
                .arg_named("nonce", None::<&Buffer<u32>>)
                .arg_named("min_zeros", None::<&Buffer<u32>>)
                .arg_named("solutions", None::<&Buffer<u64>>)
                .build()
                .unwrap();

            // set each buffer
            kernel.set_arg("message", Some(&salt_buffer)).unwrap();
            kernel.set_arg("nonce", Some(&nonce_buffer)).unwrap();
            kernel.set_arg("min_zeros", next_zeros as u32).unwrap();
            kernel.set_arg("solutions", &solutions_buffer).unwrap();

            // enqueue the kernel
            unsafe {
                kernel.enq().unwrap();
            };

            // calculate the current time
            let mut now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let current_time = now.as_secs();

            // we don't want to print too fast
            let print_output = current_time - previous_time >= 1;

            // clear the terminal screen
            if print_output {
                previous_time = current_time;

                // determine the number of attempts being made per second
                let work_rate: u128 = workfactor * cumulative_nonce as u128;

                display.update(work_rate, next_zeros, &found_list);
            }

            // increment the cumulative nonce (does not reset after a match)
            cumulative_nonce += 1;

            // record the start time of the work
            let work_start_time_millis = now.as_secs() * 1000 + now.subsec_nanos() as u64 / 1000000;

            // sleep for 99% of the previous work duration to conserve CPU
            if work_duration_millis != 0 {
                std::thread::sleep(std::time::Duration::from_millis(
                    work_duration_millis * 990 / 1000,
                ));
            }

            // read the solutions from the device
            solutions_buffer.read(&mut solutions).enq().unwrap();

            // record the end time of the work and compute how long the work took
            now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            work_duration_millis = (now.as_secs() * 1000 + now.subsec_nanos() as u64 / 1000000)
                - work_start_time_millis;

            // if at least one solution is found, end the loop
            if solutions[0] != 0 {
                break;
            }

            // if no solution has yet been found, increment the nonce
            nonce[0] += 1;

            // update the nonce buffer with the incremented nonce value
            nonce_buffer = Buffer::builder()
                .queue(program_queue.queue().clone())
                .flags(MemFlags::new().read_write())
                .len(1)
                .copy_host_slice(&nonce)
                .build()
                .unwrap();
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
            let address = <&Address>::try_from(&res[12..]).unwrap();

            // count total and leading zero bytes
            let mut leading = 0;
            for (i, &b) in address.iter().enumerate() {
                if b != 0 && leading == 0 {
                    // set leading on finding non-zero byte
                    leading = i;
                    break;
                }
            }

            let output = format!(
                "0x{}{}{} => {} (Score: {})",
                hex::encode(config.caller),
                hex::encode(salt),
                hex::encode(solution),
                address,
                leading,
            );

            if leading >= next_zeros {
                next_zeros = leading + 1;
            }

            found_list.push(output);
        }
    }
}

fn mk_kernel_src(config: &AppConfig) -> String {
    let mut src = String::with_capacity(2048 + KERNEL_SRC.len());

    let factory = config.factory.iter();
    let caller = config.caller.iter();
    let hash = config.codehash.iter();
    let hash = hash.enumerate().map(|(i, x)| (i + 52, x));

    for (i, x) in factory.chain(caller).enumerate().chain(hash) {
        writeln!(src, "#define S_{} {}u", i + 1, x).unwrap();
    }

    src.push_str(KERNEL_SRC);

    src
}
