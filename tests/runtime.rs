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

#[test]
fn remote_request_keeps_limits_and_returns_fallback_with_exit_two() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let body = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let head = std::str::from_utf8(&request[..end]).unwrap();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break serde_json::from_slice::<serde_json::Value>(
                        &request[end + 4..end + 4 + length],
                    )
                    .unwrap();
                }
            }
        };
        assert_eq!(body["min_runtime_secs"], 30);
        assert_eq!(body["max_runtime_secs"], 20);
        let response = serde_json::json!({"cache_hit": false, "found": true,
            "salt": format!("0x{}", "44".repeat(32)), "address": format!("0x{}", "55".repeat(20)),
            "score": 0, "runtime_ms": 20_123})
        .to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_salty"))
        .args([
            "mine",
            "--remote-server",
            &endpoint,
            "--caller",
            "0x2222222222222222222222222222222222222222",
            "--codehash",
            "0x3333333333333333333333333333333333333333333333333333333333333333",
            "--zeros",
            "6",
            "--min-runtime-secs",
            "30",
            "--max-runtime-secs",
            "20",
            "--abi",
        ])
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result = hex::decode(String::from_utf8(output.stdout).unwrap().trim()).unwrap();
    assert_eq!(&result[..32], &[0x44; 32]);
    assert_eq!(&result[32..44], &[0; 12]);
    assert_eq!(&result[44..64], &[0x55; 20]);
    assert_eq!(&result[64..], &[0; 32]);
}
