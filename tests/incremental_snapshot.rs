#![cfg(unix)]

use std::{collections::HashSet, fs, os::unix::fs::MetadataExt, path::Path, process::Command};

use snapshot_block_device::{BlockDevice, Device};
use tempfile::tempdir;

const SIZE: usize = 1 << 40;
const BASE_BYTES: usize = 64 << 20;
const CHUNK: usize = 64 << 10;
const BLOCK: usize = 4096;
const GROWTH_LIMIT: u64 = 2 << 20;
const HOLE: usize = SIZE / 2 + 2 * CHUNK + 17;

struct Patch {
    offset: usize,
    data: Vec<u8>,
}

fn fill_pattern(buf: &mut [u8], seed: usize) {
    let mut state = (seed as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for chunk in buf.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
    }
}

fn chunk_offset(index: usize) -> usize {
    index * (SIZE / (BASE_BYTES / CHUNK))
}

fn seeded_device() -> Device {
    let mut device = Device::new(SIZE).unwrap();
    let mut buf = vec![0; CHUNK];
    for index in 0..BASE_BYTES / CHUNK {
        fill_pattern(&mut buf, index);
        device.write(chunk_offset(index), &buf).unwrap();
    }
    device
}

// 所有快照共同计数，避免把硬链接引用的父层内容重复计算。
// 不跟随符号链接：被引用的历史快照已经位于同一个测试根目录中。
fn allocated_bytes(root: &Path) -> u64 {
    fn visit(path: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
        let metadata = fs::symlink_metadata(path).unwrap();
        if !seen.insert((metadata.dev(), metadata.ino())) {
            return 0;
        }
        let mut total = metadata.blocks() * 512;
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                total += visit(&entry.unwrap().path(), seen);
            }
        }
        total
    }
    visit(root, &mut HashSet::new())
}

fn assert_growth(root: &Path, before: u64) {
    let after = allocated_bytes(root);
    assert!(
        after <= before + GROWTH_LIMIT,
        "snapshot storage grew by {} bytes; limit is {GROWTH_LIMIT} bytes",
        after.saturating_sub(before)
    );
}

fn snapshot_with_budget(device: &mut Device, root: &Path, dst: &Path) {
    let before = allocated_bytes(root);
    device.snapshot(dst).unwrap();
    assert!(dst.is_dir());
    assert_growth(root, before);
}

fn write_and_snapshot(device: &mut Device, patch: &Patch, root: &Path, dst: &Path) {
    // 从 write 之前开始统计，不能通过提前写入快照目录绕过增量检查。
    let before = allocated_bytes(root);
    device.write(patch.offset, &patch.data).unwrap();
    device.snapshot(dst).unwrap();
    assert!(dst.is_dir());
    assert_growth(root, before);
}

fn apply_patches(offset: usize, buf: &mut [u8], patches: &[Patch]) {
    for patch in patches {
        let start = offset.max(patch.offset);
        let end = (offset + buf.len()).min(patch.offset + patch.data.len());
        if start < end {
            buf[start - offset..end - offset]
                .copy_from_slice(&patch.data[start - patch.offset..end - patch.offset]);
        }
    }
}

fn assert_read(device: &Device, offset: usize, expected: &[u8], actual: &mut [u8]) {
    assert_eq!(actual.len(), expected.len());
    actual.fill(0xCC);
    device.read(offset, actual).unwrap();
    if let Some(index) = actual.iter().zip(expected).position(|(a, b)| a != b) {
        panic!("data mismatch at offset {}", offset + index);
    }
}

fn verify(device: &Device, patches: &[Patch]) {
    let mut expected = vec![0; CHUNK];
    let mut actual = vec![0; CHUNK];
    // 流式校验全部 64 MiB 基础数据及其修改，不保留完整参考副本。
    for index in 0..BASE_BYTES / CHUNK {
        let offset = chunk_offset(index);
        fill_pattern(&mut expected, index);
        apply_patches(offset, &mut expected, patches);
        assert_read(device, offset, &expected, &mut actual);
    }
    // 另一个修改区域位于原本未写入的空洞中，连同前后零值一起检查。
    expected.fill(0);
    apply_patches(HOLE - 16, &mut expected, patches);
    assert_read(device, HOLE - 16, &expected, &mut actual);
    assert_read(device, SIZE - 32, &[0; 32], &mut [0; 32]);
    device.read(SIZE, &mut []).unwrap();
    assert!(device.read(SIZE, &mut [0]).is_err());
}

#[test]
fn unchanged_snapshots_reuse_existing_data() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base");
    let mut paths = vec![base.clone()];
    {
        let mut device = seeded_device();
        device.snapshot(&base).unwrap();
        assert!(base.is_dir());
        for generation in 0..3 {
            let path = dir.path().join(format!("unchanged-{generation}"));
            snapshot_with_budget(&mut device, dir.path(), &path);
            paths.push(path);
        }
        verify(&device, &[]);
    }
    // 重新打开旧快照后，没有修改的设备也不应再次完整保存数据。
    {
        let mut device = Device::open(&base).unwrap();
        let path = dir.path().join("reopened-unchanged");
        snapshot_with_budget(&mut device, dir.path(), &path);
        paths.push(path);
    }
    for path in paths {
        verify(&Device::open(&path).unwrap(), &[]);
    }
}

#[test]
fn small_updates_and_zeroes_survive_an_incremental_chain() {
    const CHILD_ROOT: &str = "SNAPSHOT_INCREMENTAL_TEST_CHILD_ROOT";
    let mut data = vec![0; BLOCK];
    fill_pattern(&mut data, 12345);
    let patches = [
        Patch {
            offset: chunk_offset(3),
            data: vec![0; BLOCK],
        },
        Patch {
            offset: chunk_offset(3) + BLOCK - 3,
            data: data.clone(),
        },
        Patch { offset: HOLE, data },
        Patch {
            offset: HOLE,
            data: vec![0; BLOCK],
        },
    ];
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = Path::new(&root);
        verify(&Device::open(&root.join("base")).unwrap(), &[]);
        for generation in 0..patches.len() {
            let path = root.join(format!("generation-{generation}"));
            verify(&Device::open(&path).unwrap(), &patches[..=generation]);
        }
        return;
    }
    let dir = tempdir().unwrap();
    let base = dir.path().join("base");
    let mut paths = vec![base.clone()];
    {
        let mut device = seeded_device();
        device.snapshot(&base).unwrap();
        for (generation, patch) in patches.iter().enumerate() {
            let path = dir.path().join(format!("generation-{generation}"));
            write_and_snapshot(&mut device, patch, dir.path(), &path);
            verify(&device, &patches[..=generation]);
            // 交替从刚生成的快照恢复，兼顾连续 snapshot 和 open 后继续修改。
            if generation % 2 == 0 {
                device = Device::open(&path).unwrap();
            }
            paths.push(path);
        }
    }
    // 依次打开每一代，确保父层未被修改，且最新修改优先于旧数据。
    for (generation, path) in paths.iter().enumerate() {
        verify(&Device::open(path).unwrap(), &patches[..generation]);
    }
    // 独立进程恢复所有层，排除用进程内状态代替持久化父层引用和增量数据。
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "small_updates_and_zeroes_survive_an_incremental_chain",
            "--nocapture",
        ])
        .env(CHILD_ROOT, dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn branches_from_an_old_snapshot_store_only_their_changes() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base");
    let left_path = dir.path().join("left");
    let right_path = dir.path().join("right");
    let next_path = dir.path().join("left-next");
    {
        let mut device = seeded_device();
        device.snapshot(&base).unwrap();
    }
    let left_patches = [
        Patch {
            offset: chunk_offset(7),
            data: vec![0; BLOCK],
        },
        Patch {
            offset: chunk_offset(7) + BLOCK - 3,
            data: vec![0xCD; BLOCK],
        },
    ];
    let right_patch = Patch {
        offset: chunk_offset(7),
        data: vec![0xAB; BLOCK],
    };
    {
        let mut left = Device::open(&base).unwrap();
        let mut right = Device::open(&base).unwrap();
        write_and_snapshot(&mut left, &left_patches[0], dir.path(), &left_path);
        verify(&right, &[]);
        write_and_snapshot(&mut right, &right_patch, dir.path(), &right_path);
        verify(&left, &left_patches[..1]);
    }
    {
        let mut left = Device::open(&left_path).unwrap();
        write_and_snapshot(&mut left, &left_patches[1], dir.path(), &next_path);
    }
    verify(&Device::open(&base).unwrap(), &[]);
    verify(&Device::open(&left_path).unwrap(), &left_patches[..1]);
    verify(&Device::open(&right_path).unwrap(), &[right_patch]);
    verify(&Device::open(&next_path).unwrap(), &left_patches);
}
