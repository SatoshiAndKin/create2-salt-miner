use ocl::{
    Device, Platform,
    enums::{DeviceInfo, PlatformInfo},
};

pub fn list_devices() {
    // Information buffer
    let mut info = vec![];

    // Iterate through available OpenCL platforms
    info.push("OpenCL Platforms:".to_owned());
    for (platform_index, platform) in Platform::list().into_iter().enumerate() {
        info.push(format!("\tPlatform ID: {}", platform_index));

        // Save platform information
        for platform_info_key in &[
            PlatformInfo::Name,
            // PlatformInfo::Vendor,
            PlatformInfo::Version,
            PlatformInfo::Profile,
            // PlatformInfo::Extensions,
        ] {
            if let Ok(platform_info_value) = platform.info(*platform_info_key) {
                info.push(format!(
                    "\t{:?}: {}",
                    platform_info_key, platform_info_value
                ));
            }
        }

        // Iterate through available OpenCL devices by platform
        if let Ok(platform_devices) = Device::list_all(platform) {
            info.push("\tDevices:".to_owned());
            for (device_index, device) in platform_devices.into_iter().enumerate() {
                info.push(format!("\t\tDevice ID: {}", device_index));
                // Save device information
                for device_info_key in &[
                    DeviceInfo::Type,
                    DeviceInfo::Name,
                    DeviceInfo::Vendor,
                    DeviceInfo::DriverVersion,
                    DeviceInfo::OpenclCVersion,
                ] {
                    if let Ok(device_info_value) = device.info(*device_info_key) {
                        info.push(format!("\t\t{:?}: {}", device_info_key, device_info_value));
                    }
                }
            }
        }
    }
    // List default platform & devices
    info.push(format!(
        "Selected Platform: {:?}",
        Platform::list()
            .into_iter()
            .position(|platform| *platform == *Platform::default())
            .expect("Default platform missing?!")
    ));

    // Print collected information
    println!("{}", info.join("\n"));
}
