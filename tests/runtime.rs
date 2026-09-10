use alloy_primitives::{Address, B256, hex};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires a native Metal or OpenCL device"]
fn native_timed_mining_qualifies_or_returns_fallback() {
    for (target, minimum, maximum, exit_code) in [(1, 1, 3, 0), (21, 3, 1, 2)] {
        let start = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_salty"))
            .args([
                "mine",
                "--factory",
                "0x1111111111111111111111111111111111111111",
                "--caller",
                "0x2222222222222222222222222222222222222222",
                "--codehash",
                "0x3333333333333333333333333333333333333333333333333333333333333333",
                "--worksize",
                "1048576",
                "--zeros",
                &target.to_string(),
                "--min-runtime-secs",
                &minimum.to_string(),
                "--max-runtime-secs",
                &maximum.to_string(),
                "--abi",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(exit_code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(start.elapsed() >= Duration::from_secs(1));
        let encoded = hex::decode(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
        assert_eq!(encoded.len(), 96);
        let salt = B256::from_slice(&encoded[..32]);
        assert_eq!(&salt[..20], &[0x22; 20]);
        let address = Address::from([0x11; 20]).create2(salt, B256::from([0x33; 32]));
        assert_eq!(&encoded[32..44], &[0; 12]);
        assert_eq!(&encoded[44..64], address.as_slice());
        assert_eq!(&encoded[64..95], &[0; 31]);
        let score = address.iter().filter(|&&byte| byte == 0).count();
        assert_eq!(usize::from(encoded[95]), score);
        assert_eq!(score >= target, exit_code == 0);
    }
}
