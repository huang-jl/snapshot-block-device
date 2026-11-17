use snapshot_block_device::{BlockDevice, Device};

fn read(device: &Device, offset: usize, length: usize) -> Vec<u8> {
    let mut buf = vec![0xCC; length];
    device.read(offset, &mut buf).unwrap();
    buf
}

#[test]
fn new_device_is_zero_filled() {
    let device = Device::new(8193).unwrap();
    assert_eq!(read(&device, 0, 8193).as_slice(), &[0; 8193]);
    assert_eq!(read(&device, 4093, 7).as_slice(), &[0; 7]);
}

#[test]
fn empty_device_accepts_only_empty_io_at_zero() {
    let mut device = Device::new(0).unwrap();
    device.read(0, &mut []).unwrap();
    device.write(0, &[]).unwrap();
    assert!(device.read(0, &mut [0]).is_err());
    assert!(device.write(0, &[1]).is_err());
    assert!(device.read(1, &mut []).is_err());
    assert!(device.write(1, &[]).is_err());
}

#[test]
fn writes_preserve_surrounding_bytes() {
    let mut device = Device::new(16).unwrap();
    device.write(3, &[1, 2, 3, 4, 5]).unwrap();
    let mut expected = vec![0; 16];
    expected[3..8].copy_from_slice(&[1, 2, 3, 4, 5]);
    assert_eq!(read(&device, 0, 16).as_slice(), expected);
    assert_eq!(read(&device, 4, 3).as_slice(), &[2, 3, 4]);
}

#[test]
fn first_and_last_bytes_are_writable() {
    let mut device = Device::new(17).unwrap();
    device.write(0, &[11]).unwrap();
    device.write(16, &[22]).unwrap();
    assert_eq!(read(&device, 0, 1).as_slice(), &[11]);
    assert_eq!(read(&device, 1, 15).as_slice(), &[0; 15]);
    assert_eq!(read(&device, 16, 1).as_slice(), &[22]);
}

#[test]
fn overlapping_writes_replace_only_the_requested_range() {
    let mut device = Device::new(12).unwrap();
    device.write(2, &[1, 2, 3, 4, 5, 6]).unwrap();
    device.write(4, &[9, 8, 7]).unwrap();
    assert_eq!(
        read(&device, 0, 12).as_slice(),
        &[0, 0, 1, 2, 9, 8, 7, 6, 0, 0, 0, 0]
    );
}

#[test]
fn unaligned_io_can_cross_multiple_block_boundaries() {
    let mut device = Device::new(4 * 4096 + 13).unwrap();
    let payload: Vec<u8> = (0..9001).map(|i| (i % 251) as u8).collect();
    let mut expected = vec![0; 4 * 4096 + 13];
    device.write(4093, &payload).unwrap();
    expected[4093..4093 + payload.len()].copy_from_slice(&payload);
    assert_eq!(read(&device, 0, expected.len()).as_slice(), expected);
    assert_eq!(read(&device, 4095, 8195).as_slice(), &expected[4095..12290]);
}

#[test]
fn zero_writes_erase_existing_data() {
    let mut device = Device::new(8192).unwrap();
    device.write(0, &[0xAB; 8192]).unwrap();
    device.write(2048, &[0; 4096]).unwrap();
    assert_eq!(read(&device, 0, 2048).as_slice(), &[0xAB; 2048]);
    assert_eq!(read(&device, 2048, 4096).as_slice(), &[0; 4096]);
    assert_eq!(read(&device, 6144, 2048).as_slice(), &[0xAB; 2048]);
}

#[test]
fn empty_io_is_valid_up_to_and_including_the_end() {
    let mut device = Device::new(8).unwrap();
    device.write(0, &[7; 8]).unwrap();
    for offset in [0, 3, 8] {
        device.read(offset, &mut []).unwrap();
        device.write(offset, &[]).unwrap();
    }
    assert_eq!(read(&device, 0, 8).as_slice(), &[7; 8]);
}

#[test]
fn invalid_reads_return_errors_including_integer_overflow() {
    let device = Device::new(8).unwrap();
    for (offset, length) in [(8, 1), (7, 2), (9, 0), (usize::MAX, 2)] {
        assert!(
            device.read(offset, &mut vec![0; length]).is_err(),
            "offset={offset}, length={length}"
        );
    }
}

#[test]
fn invalid_writes_do_not_modify_the_device() {
    let mut device = Device::new(8).unwrap();
    device.write(0, &[3; 8]).unwrap();
    for (offset, buf) in [
        (8, &[1][..]),
        (7, &[1, 2][..]),
        (9, &[][..]),
        (usize::MAX, &[1, 2][..]),
    ] {
        assert!(device.write(offset, buf).is_err());
        assert_eq!(read(&device, 0, 8).as_slice(), &[3; 8]);
    }
}

#[test]
fn read_fills_only_the_provided_buffer_slice() {
    let mut device = Device::new(8).unwrap();
    device.write(0, b"abcdefgh").unwrap();
    let mut buf = [0xCC; 8];
    device.read(2, &mut buf[2..6]).unwrap();
    assert_eq!(&buf, &[0xCC, 0xCC, b'c', b'd', b'e', b'f', 0xCC, 0xCC]);
    // 同一缓冲区可以复用，读取零值区域也必须完整填充。
    device.write(2, &[0; 4]).unwrap();
    device.read(2, &mut buf[2..6]).unwrap();
    assert_eq!(&buf, &[0xCC, 0xCC, 0, 0, 0, 0, 0xCC, 0xCC]);
}

#[test]
fn separate_devices_are_independent() {
    let mut first = Device::new(8).unwrap();
    let second = Device::new(8).unwrap();
    first.write(0, &[1; 8]).unwrap();
    assert_eq!(read(&second, 0, 8).as_slice(), &[0; 8]);
}

#[test]
fn deterministic_operations_match_a_byte_vector() {
    let size = 3 * 4096 + 17;
    let mut device = Device::new(size).unwrap();
    let mut expected = vec![0; size];
    // 固定序列，无额外随机数依赖，失败可复现。
    let mut state = 0x1234_5678_u64;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        state
    };
    for step in 0..300 {
        let offset = (next() % (size as u64 + 1)) as usize;
        let length = (next() % ((size - offset).min(5000) as u64 + 1)) as usize;
        if step % 3 != 0 {
            let payload: Vec<u8> = (0..length).map(|_| (next() >> 32) as u8).collect();
            device.write(offset, &payload).unwrap();
            expected[offset..offset + length].copy_from_slice(&payload);
        } else {
            assert_eq!(
                read(&device, offset, length).as_slice(),
                &expected[offset..offset + length],
                "step={step}, offset={offset}, length={length}"
            );
        }
    }
    assert_eq!(read(&device, 0, size).as_slice(), expected);
}
