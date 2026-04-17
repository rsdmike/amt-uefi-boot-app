use crate::error::{Error, Result};

/// AMTHI client UUID (little-endian)
const AMTHI_UUID: [u8; 16] = [
    0x28, 0x00, 0xF8, 0x12, 0xB7, 0xB4, 0x2D, 0x4B,
    0xAC, 0xA8, 0x46, 0xE0, 0xFF, 0x65, 0x81, 0x4C,
];

// IOCTL_MEI_CONNECT_CLIENT = _IOWR('H', 0x01, struct mei_connect_client_data)
// struct is a 16-byte union (uuid_le in / mei_client out).
// _IOC encoding: dir(2)<<30 | size(14)<<16 | type(8)<<8 | nr(8)
const IOCTL_MEI_CONNECT_CLIENT: libc::c_ulong = 0xC010_4801;

pub struct HeciContext {
    fd: libc::c_int,
    pub max_msg_len: u32,
}

impl HeciContext {
    pub fn new() -> Result<Self> {
        let mut ctx = HeciContext {
            fd: -1,
            max_msg_len: 0,
        };
        ctx.open_device()?;
        Ok(ctx)
    }

    /// Re-open the MEI device. Close must be called first by caller if needed.
    ///
    /// # Safety
    /// Performs raw file descriptor operations.
    pub unsafe fn init(&mut self) -> Result<()> {
        if self.fd >= 0 {
            libc::close(self.fd);
            self.fd = -1;
        }
        self.open_device()
    }

    fn open_device(&mut self) -> Result<()> {
        // Try /dev/mei0 first (modern kernels), fall back to /dev/mei (older).
        let paths: [&[u8]; 2] = [b"/dev/mei0\0", b"/dev/mei\0"];
        for path in paths.iter() {
            let fd = unsafe {
                libc::open(
                    path.as_ptr() as *const libc::c_char,
                    libc::O_RDWR | libc::O_CLOEXEC,
                )
            };
            if fd >= 0 {
                self.fd = fd;
                dprintln!("HECI: Opened {}", core::str::from_utf8(&path[..path.len() - 1]).unwrap_or("?"));
                return Ok(());
            }
        }
        let err = errno();
        dprintln!("HECI: open /dev/mei* failed (errno {})", err);
        if err == libc::EACCES || err == libc::EPERM {
            Err(Error::AccessDenied)
        } else {
            Err(Error::NotFound)
        }
    }

    fn connect_client_by_uuid(&mut self, uuid: &[u8; 16]) -> Result<()> {
        // 16-byte union: write UUID in, kernel writes back struct mei_client
        // (max_msg_length:u32 + protocol_version:u8 + reserved[3]).
        let mut buf = [0u8; 16];
        buf.copy_from_slice(uuid);

        let rc = unsafe {
            libc::ioctl(
                self.fd,
                IOCTL_MEI_CONNECT_CLIENT,
                buf.as_mut_ptr() as *mut libc::c_void,
            )
        };
        if rc < 0 {
            let err = errno();
            dprintln!("HECI: IOCTL_MEI_CONNECT_CLIENT failed (errno {})", err);
            return match err {
                libc::ENOTTY | libc::ENODEV => Err(Error::NotFound),
                libc::EACCES | libc::EPERM => Err(Error::AccessDenied),
                _ => Err(Error::DeviceError),
            };
        }

        self.max_msg_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        dprintln!("HECI: Connected, max_msg_len={}", self.max_msg_len);
        Ok(())
    }

    pub fn connect_amthi(&mut self) -> Result<()> {
        dprintln!("HECI: Connecting to AMTHI client");
        self.connect_client_by_uuid(&AMTHI_UUID)
    }

    pub fn connect_client(&mut self, target_uuid: &[u8; 16]) -> Result<()> {
        dprintln!("HECI: Connecting to ME client");
        self.connect_client_by_uuid(target_uuid)
    }

    /// Send data to the connected ME client.
    pub fn send(&self, data: &[u8]) -> Result<()> {
        let n = unsafe {
            libc::write(
                self.fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
            )
        };
        if n < 0 {
            dprintln!("HECI: write failed (errno {})", errno());
            return Err(Error::DeviceError);
        }
        dprintln!("  [heci_send] Sent {} bytes", n);
        Ok(())
    }

    /// Receive data from the connected ME client.
    /// MEI requires the read buffer to be >= max_msg_len.
    pub fn receive(&self, buf: &mut [u8]) -> Result<u32> {
        let read_size = self.max_msg_len.max(4096) as usize;
        let mut tmp_buf = vec![0u8; read_size];

        let n = unsafe {
            libc::read(
                self.fd,
                tmp_buf.as_mut_ptr() as *mut libc::c_void,
                read_size,
            )
        };
        if n < 0 {
            dprintln!("HECI: read failed (errno {})", errno());
            return Err(Error::DeviceError);
        }
        let bytes = n as u32;
        dprintln!("  [heci_recv] Got {} bytes", bytes);

        let copy_len = (n as usize).min(buf.len());
        buf[..copy_len].copy_from_slice(&tmp_buf[..copy_len]);
        Ok(bytes)
    }

    pub fn close(&mut self) {
        dprintln!("HECI: Closing");
        if self.fd >= 0 {
            unsafe { libc::close(self.fd); }
            self.fd = -1;
        }
        self.max_msg_len = 0;
    }
}

impl Drop for HeciContext {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe { libc::close(self.fd); }
            self.fd = -1;
        }
    }
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}
