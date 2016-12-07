use std::cell::UnsafeCell;
use std::fs::File;
use std::io::{Read, Write, Result};
use std::mem;
use std::os::unix::io::{AsRawFd, RawFd};
use std::slice;

/// Redox domain socket
pub struct Socket {
    file: UnsafeCell<File>
}

unsafe impl Send for Socket {}
unsafe impl Sync for Socket {}

impl Socket {
    pub fn open(path: &str) -> Result<Socket> {
        let file = try!(File::open(path));
        Ok(Socket {
            file: UnsafeCell::new(file)
        })
    }

    pub fn create(path: &str) -> Result<Socket> {
        let file = try!(File::create(path));
        Ok(Socket {
            file: UnsafeCell::new(file)
        })
    }

    pub fn receive(&self, buf: &mut [u8]) -> Result<usize> {
        unsafe { (*self.file.get()).read(buf) }
    }

    pub fn receive_type<T: Copy>(&self, buf: &mut [T]) -> Result<usize> {
        self.receive(unsafe { slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * mem::size_of::<T>()) }).map(|count| count/mem::size_of::<T>())
    }

    pub fn send(&self, buf: &[u8]) -> Result<usize> {
        unsafe { (*self.file.get()).write(buf) }
    }

    pub fn sync(&self) -> Result<()> {
        unsafe { (*self.file.get()).sync_data() }
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        unsafe { (*self.file.get()).as_raw_fd() }
    }
}
