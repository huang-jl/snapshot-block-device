use std::path::Path;

use crate::{BlockDevice, Result};

pub struct Device {
    // TODO: 定义设备状态。
}

impl BlockDevice for Device {
    fn new(_size: usize) -> Result<Self> {
        todo!("create a zero-filled device")
    }

    fn read(&self, _offset: usize, _buf: &mut [u8]) -> Result<()> {
        todo!("read bytes from the device")
    }

    fn write(&mut self, _offset: usize, _buf: &[u8]) -> Result<()> {
        todo!("write bytes to the device")
    }

    fn snapshot(&mut self, _dst: &Path) -> Result<()> {
        todo!("persist the current device state")
    }

    fn open(_layer: &Path) -> Result<Self> {
        todo!("restore a writable device from a snapshot")
    }
}
