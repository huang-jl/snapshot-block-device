# 支持快照的块设备：第一阶段

在 Linux 64 位环境下，实现支持持久化快照的用户态虚拟块设备，无需内核驱动或挂载。
完成 `src/device.rs` 中的 `BlockDevice` 实现：

```rust
fn new(size: usize) -> Result<Self>;
fn read(&self, offset: usize, buf: &mut [u8]) -> Result<()>;
fn write(&mut self, offset: usize, buf: &[u8]) -> Result<()>;
fn snapshot(&mut self, dst: &Path) -> Result<()>;
fn open(layer: &Path) -> Result<Self>;
```

## 读写

- 设备大小固定，初始内容全为零，允许大小为零。
- 偏移和长度以字节为单位，支持任意对齐的读写。
- `read` 从 `offset` 开始读取 `buf.len()` 字节，成功时完整填充 `buf`。
- `write` 只覆盖指定范围，其余内容不变。
- 越界或范围计算溢出时返回错误；越界写入不得修改设备。
- 空读写允许偏移位于 `0..=size`，超过 `size` 仍然返回错误。
- 不同设备实例的写入互不影响。

## 快照

- `snapshot` 创建 `dst` 目录，在其中保存设备大小和内容，格式自行设计。
- 调用者保证父目录存在；若 `dst` 已存在，返回错误，不修改目标或设备内容。
- 快照成功后，当前设备可以继续读写，后续修改不影响已有快照。
- 创建快照的进程退出后，仍可通过 `open` 从快照目录恢复设备。
- 恢复后的设备可以继续读写和创建快照；从同一快照打开的多个实例互不影响，也不能修改原快照。
- 打开不存在的路径时返回错误。测试会保留所有快照目录及其路径。

本阶段不限制实现方式和资源开销，不要求并发访问或掉电一致性。

## 测试

```bash
cargo test --test basic
cargo test --test snapshot
```

骨架中的 `todo!()` 需要自行实现；完成后应通过全部测试。
