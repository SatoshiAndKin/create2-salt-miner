use std::fmt::Write;

const THRESHOLDS: usize = 22;
const SCORE_PROBE: &str = r#"
#if defined(METAL_BACKEND)
kernel void checkScore(device const uint *inputs [[buffer(0)]],
                       device uint *outputs [[buffer(1)]],
                       uint index [[thread_position_in_grid]]) {
#else
__kernel void checkScore(__global const uint *inputs, __global uint *outputs) {
  uint index = get_global_id(0);
#endif
  uint words[5];
  for (uint i = 0; i < 5; ++i)
    words[i] = inputs[5 * (index / 22) + i];
  outputs[index] = hasZeroBytes((THREAD const uchar *)words, index % 22);
}
"#;

fn cases() -> Vec<[u32; 5]> {
    let mut cases = Vec::new();
    // Every byte value at every position, against dense-zero and no-zero
    // backgrounds. Adjacent zero/one bytes expose cross-byte borrow errors.
    for fill in [0_u8, 0xff] {
        for position in 0..20 {
            for value in 0..=u8::MAX {
                let mut bytes = [fill; 20];
                bytes[position] = value;
                cases.push(std::array::from_fn(|word| {
                    u32::from_le_bytes(bytes[word * 4..word * 4 + 4].try_into().unwrap())
                }));
            }
        }
    }
    for word in [0x0001_0001, 0x0100_0100, 0x0080_007f, 0x8000_7f00] {
        cases.push([word; 5]);
    }
    cases
}

#[test]
#[ignore = "requires a native Metal or OpenCL device"]
fn native_zero_byte_scoring_matches_cpu_for_all_byte_values() {
    let cases = cases();
    let mut source = String::new();
    // Hash inputs are unused by this scoring probe. Compile the actual shared
    // kernel, then call its scorer directly on adversarial address bytes.
    for index in 1..=84 {
        writeln!(source, "#define S_{index} 0u").unwrap();
    }
    source.push_str(include_str!("../src/kernels/keccak256.cl"));
    source.push_str(SCORE_PROBE);
    let actual = score_on_gpu(&source, &cases);
    assert_eq!(actual.len(), cases.len() * THRESHOLDS);
    for (index, case) in cases.iter().enumerate() {
        let score = case
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .filter(|&byte| byte == 0)
            .count();
        for threshold in 0..THRESHOLDS {
            assert_eq!(
                actual[index * THRESHOLDS + threshold],
                u32::from(score >= threshold),
                "case {index}, threshold {threshold}, input {case:08x?}"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn score_on_gpu(source: &str, cases: &[[u32; 5]]) -> Vec<u32> {
    use ocl::{Buffer, ProQue};
    let count = cases.len() * THRESHOLDS;
    let queue = ProQue::builder().src(source).dims(count).build().unwrap();
    let inputs = Buffer::<u32>::builder()
        .queue(queue.queue().clone())
        .copy_host_slice(cases.as_flattened())
        .len(cases.len() * 5)
        .build()
        .unwrap();
    let outputs = Buffer::<u32>::builder()
        .queue(queue.queue().clone())
        .len(count)
        .build()
        .unwrap();
    let kernel = queue
        .kernel_builder("checkScore")
        .arg(&inputs)
        .arg(&outputs)
        .build()
        .unwrap();
    unsafe { kernel.enq().unwrap() };
    let mut result = vec![0_u32; count];
    outputs.read(&mut result).enq().unwrap();
    result
}

#[cfg(target_os = "macos")]
fn score_on_gpu(source: &str, cases: &[[u32; 5]]) -> Vec<u32> {
    use metal::{CompileOptions, Device, MTLCommandBufferStatus, MTLResourceOptions, MTLSize};
    objc::rc::autoreleasepool(|| {
        let count = cases.len() * THRESHOLDS;
        let device = Device::system_default().expect("native Metal device");
        let options = CompileOptions::new();
        options.set_fast_math_enabled(true);
        let library = device
            .new_library_with_source(&format!("#define METAL_BACKEND 1\n{source}"), &options)
            .unwrap();
        let function = library.get_function("checkScore", None).unwrap();
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .unwrap();
        let inputs = device.new_buffer_with_data(
            cases.as_ptr().cast(),
            std::mem::size_of_val(cases) as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let outputs = device.new_buffer(
            (count * std::mem::size_of::<u32>()) as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let queue = device.new_command_queue();
        let commands = queue.new_command_buffer();
        let encoder = commands.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&pipeline);
        encoder.set_buffer(0, Some(&inputs), 0);
        encoder.set_buffer(1, Some(&outputs), 0);
        encoder.dispatch_threads(
            MTLSize {
                width: count as u64,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: pipeline.thread_execution_width(),
                height: 1,
                depth: 1,
            },
        );
        encoder.end_encoding();
        commands.commit();
        commands.wait_until_completed();
        assert_eq!(commands.status(), MTLCommandBufferStatus::Completed);
        unsafe { std::slice::from_raw_parts(outputs.contents().cast::<u32>(), count).to_vec() }
    })
}
