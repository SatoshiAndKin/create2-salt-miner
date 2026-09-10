use eyre::Result;
use ocl::{Buffer, Context, Device, Kernel, Platform, Program, Queue};
use ocl::enums::{KernelWorkGroupInfo, ProgramBuildInfo};
use std::fmt::Write;

fn main() -> Result<()> {
    let platform = Platform::default();
    let device = Device::first(platform)?;
    let context = Context::builder().platform(platform).devices(device).build()?;
    let queue = Queue::new(&context, device, None)?;
    let output = Buffer::<u64>::builder().queue(queue.clone()).len(1).build()?;
    let factory = alloy_primitives::hex::decode("0000000000FFe8B47B3e2130213B802212439497")?;
    let caller = [0_u8; 20];
    let codehash = alloy_primitives::hex::decode("64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107")?;
    for path in std::env::args().skip(1) {
        let mut source = String::new();
        for (index, byte) in factory.iter().chain(&caller).enumerate()
            .chain(codehash.iter().enumerate().map(|(i, x)| (i + 52, x))) {
            writeln!(source, "#define S_{} {}u", index + 1, byte)?;
        }
        source.push_str(&std::fs::read_to_string(&path)?);
        let program = Program::builder().devices(device).src(source)
            .cmplr_opt("-cl-nv-verbose").build(&context)?;
        let kernel = Kernel::builder().program(&program).queue(queue.clone())
            .name("hashMessage").arg(0_u32).arg(0_u32).arg(21_u32).arg(&output).build()?;
        println!("Kernel source: {path}");
        println!("{}", program.build_info(device, ProgramBuildInfo::BuildLog)?);
        for field in [KernelWorkGroupInfo::WorkGroupSize,
            KernelWorkGroupInfo::PreferredWorkGroupSizeMultiple,
            KernelWorkGroupInfo::LocalMemSize, KernelWorkGroupInfo::PrivateMemSize] {
            println!("{field:?}: {:?}", kernel.wg_info(device, field)?);
        }
    }
    Ok(())
}
