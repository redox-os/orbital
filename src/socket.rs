use std::cell::UnsafeCell;
use std::fs::File;
use std::io::{Read, Write, Result, Seek, SeekFrom};
use std::mem;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::PathBuf;
use std::slice;

/// Redox domain socket
pub struct Socket {
    file: UnsafeCell<File>
}

unsafe impl Send for Socket {}
unsafe impl Sync for Socket {}

impl Socket {
    pub fn new(fd: RawFd) -> Socket {
        Socket {
            file: UnsafeCell::new(unsafe { File::from_raw_fd(fd) })
        }
    }

    pub fn path(&self) -> Result<PathBuf> {
        unsafe { (*self.file.get()).path() }
    }

    pub fn receive(&self, buf: &mut [u8]) -> Result<usize> {
        unsafe { (*self.file.get()).read(buf) }
    }

    pub fn receive_type<T: Copy>(&self, buf: &mut [T]) -> Result<usize> {
        self.receive(unsafe { slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * mem::size_of::<T>()) }).map(|count| count/mem::size_of::<T>())
    }

    pub unsafe fn seek(&self, from: SeekFrom) -> Result<u64> {
        (*self.file.get()).seek(from)
    }

    pub fn send(&self, buf: &[u8]) -> Result<usize> {
        unsafe { (*self.file.get()).write(buf) }
    }

    pub fn send_type<T: Copy>(&self, buf: &[T]) -> Result<usize> {
        self.send(unsafe { slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len() * mem::size_of::<T>()) }).map(|count| count/mem::size_of::<T>())
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        unsafe { (*self.file.get()).as_raw_fd() }
    }
}
