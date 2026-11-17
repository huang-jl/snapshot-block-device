use snapshot_block_device::{BlockDevice, Device};
use tempfile::tempdir;

const SIZE: usize = 1 << 40; // 1 TiB，测试要求 64 位环境。
const WRITE_LIMIT: usize = 128 << 20;
const CHUNK_SIZE: usize = 64 << 10;

fn fill_pattern(buf: &mut [u8], seed: usize) {
    let mut state = (seed as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for byte in buf {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
}

fn assert_read(device: &Device, offset: usize, expected: &[u8], actual: &mut [u8]) {
    assert_eq!(actual.len(), expected.len());
    actual.fill(0xCC);
    device.read(offset, actual).unwrap();
    if let Some(index) = actual.iter().zip(expected).position(|(a, b)| a != b) {
        panic!(
            "data mismatch at offset {}: expected {}, got {}",
            offset + index,
            expected[index],
            actual[index]
        );
    }
}

fn assert_zeros(device: &Device, offset: usize) {
    assert_read(device, offset, &[0; 32], &mut [0; 32]);
}

fn assert_bounds(device: &mut Device) {
    device.read(SIZE, &mut []).unwrap();
    device.write(SIZE, &[]).unwrap();
    assert!(device.read(SIZE, &mut [0]).is_err());
    assert!(device.read(SIZE + 1, &mut []).is_err());
    assert!(device.read(SIZE - 1, &mut [0; 2]).is_err());
    assert!(device.write(SIZE, &[1]).is_err());
    assert!(device.write(SIZE + 1, &[]).is_err());
    assert!(device.write(SIZE - 1, &[1; 2]).is_err());
}

#[test]
fn unwritten_large_device_and_snapshot_preserve_size_and_zeroes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("empty");
    {
        let mut device = Device::new(SIZE).unwrap();
        assert_bounds(&mut device);
        for offset in [0, (1 << 32) - 16, SIZE / 2, SIZE - 32] {
            assert_zeros(&device, offset);
        }
        device.snapshot(&path).unwrap();
        assert!(path.is_dir());
    }
    let mut restored = Device::open(&path).unwrap();
    assert_bounds(&mut restored);
    for offset in [0, (1 << 32) - 16, SIZE / 2, SIZE - 32] {
        assert_zeros(&restored, offset);
    }
}

#[test]
fn scattered_unaligned_writes_and_overwrites_preserve_other_regions() {
    let mut device = Device::new(SIZE).unwrap();
    // 前两个地址低 32 位相同，可以发现偏移被截断成 u32 的实现。
    let offsets = [17, (1 << 32) + 17, SIZE / 2 - 4093, SIZE - 8193];
    let mut expected = vec![vec![0; 8193]; offsets.len()];
    let mut actual = vec![0; 8193];
    for (index, &offset) in offsets.iter().enumerate() {
        fill_pattern(&mut expected[index], index);
        device.write(offset, &expected[index]).unwrap();
    }
    // 在高地址执行未对齐的局部覆盖，再将另一区域完整写零。
    device.write(offsets[1] + 4093, b"overwrite").unwrap();
    expected[1][4093..4102].copy_from_slice(b"overwrite");
    expected[2].fill(0);
    device.write(offsets[2], &expected[2]).unwrap();
    assert_bounds(&mut device);
    for (index, &offset) in offsets.iter().enumerate() {
        assert_read(&device, offset, &expected[index], &mut actual);
        assert_read(&device, offset - 1, &[0], &mut [0]);
        if offset + actual.len() < SIZE {
            assert_zeros(&device, offset + actual.len());
        }
    }
    assert_zeros(&device, SIZE / 4);
}

fn chunk_offset(index: usize) -> usize {
    let stride = SIZE / (WRITE_LIMIT / CHUNK_SIZE);
    index * stride + (index * 37) % 4096
}

fn verify_written_chunks(device: &Device, expected: &mut [u8], actual: &mut [u8]) {
    for index in 0..WRITE_LIMIT / CHUNK_SIZE {
        fill_pattern(expected, index);
        let offset = chunk_offset(index);
        assert_read(device, offset, expected, actual);
        if offset > 0 {
            assert_read(device, offset - 1, &[0], &mut [0]);
        }
        assert_zeros(device, offset + CHUNK_SIZE);
    }
    assert_zeros(device, SIZE - 32);
}

#[test]
fn full_128_mib_of_writes_survive_snapshot_and_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("full-write-budget");
    // 测试只保留两个 64 KiB 缓冲区，按固定种子重建期望数据。
    let mut expected = vec![0; CHUNK_SIZE];
    let mut actual = vec![0; CHUNK_SIZE];
    {
        let mut device = Device::new(SIZE).unwrap();
        for index in 0..WRITE_LIMIT / CHUNK_SIZE {
            fill_pattern(&mut expected, index);
            device.write(chunk_offset(index), &expected).unwrap();
        }
        // 累计恰好写入 128 MiB，不追加覆盖写入。
        verify_written_chunks(&device, &mut expected, &mut actual);
        device.snapshot(&path).unwrap();
        assert!(path.is_dir());
    }
    let mut restored = Device::open(&path).unwrap();
    assert_bounds(&mut restored);
    verify_written_chunks(&restored, &mut expected, &mut actual);
}

#[test]
fn large_snapshot_branches_remain_writable_and_independent() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base");
    let left_path = dir.path().join("left");
    let right_path = dir.path().join("right");
    let offset = SIZE - CHUNK_SIZE;
    let mut expected = vec![0; CHUNK_SIZE];
    let mut actual = vec![0; CHUNK_SIZE];
    fill_pattern(&mut expected, 42);
    {
        let mut original = Device::new(SIZE).unwrap();
        original.write(offset, &expected).unwrap();
        original.snapshot(&base).unwrap();
        original.write(offset, &vec![0xAB; CHUNK_SIZE]).unwrap();

        let mut left = Device::open(&base).unwrap();
        let mut right = Device::open(&base).unwrap();
        assert_read(&left, offset, &expected, &mut actual);
        assert_read(&right, offset, &expected, &mut actual);
        left.write(offset, &vec![0; CHUNK_SIZE]).unwrap();
        assert_read(&right, offset, &expected, &mut actual);
        right.write(offset + 4093, b"branch").unwrap();
        assert_read(&left, offset, &vec![0; CHUNK_SIZE], &mut actual);
        assert_read(&original, offset, &vec![0xAB; CHUNK_SIZE], &mut actual);
        left.snapshot(&left_path).unwrap();
        right.snapshot(&right_path).unwrap();
        assert!(base.is_dir() && left_path.is_dir() && right_path.is_dir());
    }
    // 保留全部目录，但所有可写实例均已销毁。
    assert_read(
        &Device::open(&base).unwrap(),
        offset,
        &expected,
        &mut actual,
    );
    assert_read(
        &Device::open(&left_path).unwrap(),
        offset,
        &vec![0; CHUNK_SIZE],
        &mut actual,
    );
    expected[4093..4099].copy_from_slice(b"branch");
    let mut restored = Device::open(&right_path).unwrap();
    assert_bounds(&mut restored);
    assert_read(&restored, offset, &expected, &mut actual);
    assert_zeros(&restored, 0);
}
