//! 支持持久化快照的用户态虚拟块设备练习。

mod device;

pub use device::Device;
pub use tempfile::TempDir;

use std::{error::Error as StdError, fmt, io, path::Path};

pub type Result<T> = std::result::Result<T, Error>;

/// 创建独立的临时工作目录，返回值销毁时自动删除目录及其内容。
/// 将 TempDir 保存在 Device 中以维持目录生命周期，不要只保存路径。
/// 持久化快照应保存到调用者提供的 dst，而非该临时目录。
pub fn create_temp_dir() -> Result<TempDir> {
    Ok(tempfile::Builder::new()
        .prefix("snapshot-block-device-")
        .tempdir()?)
}

#[derive(Debug)]
pub enum Error {
    OutOfBounds,
    InvalidSnapshot(String),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds => write!(f, "read or write exceeds device bounds"),
            Self::InvalidSnapshot(reason) => write!(f, "invalid snapshot: {reason}"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 单线程使用的、固定逻辑大小的字节寻址设备。
pub trait BlockDevice: Sized {
    /// 创建大小为 size 的设备，初始内容全部为零。允许 size 为零。
    fn new(size: usize) -> Result<Self>;

    /// 将 [offset, offset + buf.len()) 的内容完整读入 buf。
    /// 越界或范围计算溢出时返回错误；空读取允许 offset 位于 0..=size。
    fn read(&self, offset: usize, buf: &mut [u8]) -> Result<()>;

    /// 覆盖 [offset, offset + buf.len())；越界时返回错误且不得修改设备。
    /// 空写入允许 offset 位于 0..=size。
    fn write(&mut self, offset: usize, buf: &[u8]) -> Result<()>;

    /// 创建 dst 目录，并在其中持久化当前完整逻辑状态。
    /// 成功后当前设备仍可读写，后续修改不影响该快照。
    /// dst 的父目录由调用者提供；若 dst 已存在，返回错误且不修改它。
    fn snapshot(&mut self, dst: &Path) -> Result<()>;

    /// 从 layer 目录恢复快照的大小和内容，得到独立可写的设备。
    /// 对返回设备的修改不得改变快照或其他从该快照打开的设备。
    fn open(layer: &Path) -> Result<Self>;
}
