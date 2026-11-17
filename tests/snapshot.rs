use std::{fs, process::Command};

use snapshot_block_device::{BlockDevice, Device};
use tempfile::tempdir;

fn assert_state(device: &Device, expected: &[u8]) {
    let mut buf = vec![0xCC; expected.len()];
    device.read(0, &mut buf).unwrap();
    assert_eq!(buf, expected);
    device.read(expected.len(), &mut []).unwrap();
    assert!(device.read(expected.len(), &mut [0]).is_err());
}

#[test]
fn snapshot_restores_content_and_exact_size_after_drop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("snapshot");
    let mut expected = vec![0; 8199];
    expected[4093..4100].copy_from_slice(b"example");
    expected[8198] = 42;
    {
        let mut device = Device::new(expected.len()).unwrap();
        device.write(4093, b"example").unwrap();
        device.write(8198, &[42]).unwrap();
        device.snapshot(&path).unwrap();
        assert!(path.is_dir());
        assert_state(&device, &expected);
    }
    let mut restored = Device::open(&path).unwrap();
    assert_state(&restored, &expected);
    assert!(restored.write(expected.len(), &[1]).is_err());
    assert_state(&restored, &expected);
}

#[test]
fn empty_and_unwritten_devices_can_be_snapshotted() {
    let dir = tempdir().unwrap();
    for size in [0, 1, 4097] {
        let path = dir.path().join(format!("snapshot-{size}"));
        {
            let mut device = Device::new(size).unwrap();
            device.snapshot(&path).unwrap();
        }
        assert_state(&Device::open(&path).unwrap(), &vec![0; size]);
    }
}

#[test]
fn later_writes_do_not_change_earlier_snapshots() {
    let dir = tempdir().unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    let mut device = Device::new(8).unwrap();
    device.write(0, b"original").unwrap();
    device.snapshot(&first).unwrap();
    device.write(0, b"modified").unwrap();
    device.snapshot(&second).unwrap();
    device.write(0, b"latest!!").unwrap();
    assert_state(&device, b"latest!!");
    assert_state(&Device::open(&first).unwrap(), b"original");
    assert_state(&Device::open(&second).unwrap(), b"modified");
}

#[test]
fn repeated_snapshots_without_writes_preserve_content() {
    let dir = tempdir().unwrap();
    let mut device = Device::new(5).unwrap();
    device.write(0, b"hello").unwrap();
    for name in ["one", "two", "three"] {
        let path = dir.path().join(name);
        device.snapshot(&path).unwrap();
        assert_state(&Device::open(&path).unwrap(), b"hello");
    }
    assert_state(&device, b"hello");
}

#[test]
fn opened_devices_are_independent_writable_branches() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("base");
    let left_path = dir.path().join("left");
    let right_path = dir.path().join("right");
    {
        let mut original = Device::new(8).unwrap();
        original.write(0, b"abcdefgh").unwrap();
        original.snapshot(&base).unwrap();
        let mut left = Device::open(&base).unwrap();
        let mut right = Device::open(&base).unwrap();
        left.write(0, b"LEFT").unwrap();
        assert_state(&right, b"abcdefgh");
        right.write(4, b"RGHT").unwrap();
        assert_state(&left, b"LEFTefgh");
        assert_state(&original, b"abcdefgh");
        left.snapshot(&left_path).unwrap();
        right.snapshot(&right_path).unwrap();
    }
    assert_state(&Device::open(&base).unwrap(), b"abcdefgh");
    assert_state(&Device::open(&left_path).unwrap(), b"LEFTefgh");
    assert_state(&Device::open(&right_path).unwrap(), b"abcdRGHT");
}

#[test]
fn reopened_snapshots_can_form_a_chain_including_zero_overwrites() {
    let dir = tempdir().unwrap();
    let mut expected = vec![0xAB; 3 * 4096 + 7];
    let base = dir.path().join("base");
    {
        let mut device = Device::new(expected.len()).unwrap();
        device.write(0, &expected).unwrap();
        device.snapshot(&base).unwrap();
    }
    let mut previous = base.clone();
    let mut states = vec![(base, expected.clone())];
    for generation in 0..4 {
        let mut device = Device::open(&previous).unwrap();
        let offset = [4096, 4093, 0, 8195][generation];
        let payload = vec![generation as u8; 4096];
        device.write(offset, &payload).unwrap();
        expected[offset..offset + payload.len()].copy_from_slice(&payload);
        let path = dir.path().join(format!("generation-{generation}"));
        device.snapshot(&path).unwrap();
        assert_state(&device, &expected);
        states.push((path.clone(), expected.clone()));
        previous = path;
    }
    for (path, state) in states {
        assert_state(&Device::open(&path).unwrap(), &state);
    }
}

#[test]
fn existing_snapshot_targets_are_rejected_without_changes() {
    let dir = tempdir().unwrap();
    let existing_file = dir.path().join("file");
    let existing_dir = dir.path().join("directory");
    fs::write(&existing_file, b"keep file").unwrap();
    fs::create_dir(&existing_dir).unwrap();
    fs::write(existing_dir.join("marker"), b"keep directory").unwrap();
    let mut device = Device::new(4).unwrap();
    device.write(0, b"data").unwrap();
    assert!(device.snapshot(&existing_file).is_err());
    assert!(device.snapshot(&existing_dir).is_err());
    assert_eq!(fs::read(&existing_file).unwrap(), b"keep file");
    assert_eq!(
        fs::read(existing_dir.join("marker")).unwrap(),
        b"keep directory"
    );
    assert_eq!(fs::read_dir(&existing_dir).unwrap().count(), 1);
    assert_state(&device, b"data");
    let valid = dir.path().join("valid");
    device.snapshot(&valid).unwrap();
    assert_state(&Device::open(&valid).unwrap(), b"data");
}

#[test]
fn opening_a_missing_snapshot_returns_an_error() {
    let dir = tempdir().unwrap();
    assert!(Device::open(&dir.path().join("missing")).is_err());
}

#[test]
fn snapshot_survives_the_creating_process() {
    // 子进程只负责创建快照，退出后父进程恢复，排除全局内存充当持久化。
    const CHILD_PATH: &str = "SNAPSHOT_BLOCK_DEVICE_TEST_CHILD_PATH";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let mut device = Device::new(19).unwrap();
        device.write(3, b"persisted").unwrap();
        device.snapshot(std::path::Path::new(&path)).unwrap();
        return;
    }
    let dir = tempdir().unwrap();
    let path = dir.path().join("from-child");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "snapshot_survives_the_creating_process",
            "--nocapture",
        ])
        .env(CHILD_PATH, &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected = vec![0; 19];
    expected[3..12].copy_from_slice(b"persisted");
    assert_state(&Device::open(&path).unwrap(), &expected);
}
