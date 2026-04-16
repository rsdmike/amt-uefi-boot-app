use crate::error::{Error, Result};
use std::ptr;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::*;
use windows_sys::Win32::System::Threading::*;

/// MEI device interface GUID: {E2D1FF34-3458-49A9-88DA-8E6915CE9BE5}
const GUID_MEI_INTERFACE: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0xE2D1FF34,
    data2: 0x3458,
    data3: 0x49A9,
    data4: [0x88, 0xDA, 0x8E, 0x69, 0x15, 0xCE, 0x9B, 0xE5],
};

/// AMTHI client UUID (little-endian)
const AMTHI_UUID: [u8; 16] = [
    0x28, 0x00, 0xF8, 0x12, 0xB7, 0xB4, 0x2D, 0x4B,
    0xAC, 0xA8, 0x46, 0xE0, 0xFF, 0x65, 0x81, 0x4C,
];

const FILE_DEVICE_HECI: u32 = 0x8000;

fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

pub struct HeciContext {
    handle: HANDLE,
    pub max_msg_len: u32,
}

impl HeciContext {
    pub fn new() -> Result<Self> {
        let mut ctx = HeciContext {
            handle: INVALID_HANDLE_VALUE,
            max_msg_len: 0,
        };
        ctx.open_device()?;
        Ok(ctx)
    }

    pub unsafe fn init(&mut self) -> Result<()> {
        if self.handle != INVALID_HANDLE_VALUE {
            // Don't CloseHandle — Intel MEI driver bug (see rpc-go note)
            self.handle = INVALID_HANDLE_VALUE;
        }
        self.open_device()
    }

    fn open_device(&mut self) -> Result<()> {
        unsafe {
            let dev_info = SetupDiGetClassDevsW(
                &GUID_MEI_INTERFACE as *const _,
                ptr::null(),
                0 as _,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            );
            if dev_info == INVALID_HANDLE_VALUE as _ {
                dprintln!("HECI: SetupDiGetClassDevs failed");
                return Err(Error::NotFound);
            }

            let mut iface_data: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            iface_data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

            let ok = SetupDiEnumDeviceInterfaces(
                dev_info,
                ptr::null(),
                &GUID_MEI_INTERFACE as *const _,
                0,
                &mut iface_data,
            );
            if ok == 0 {
                SetupDiDestroyDeviceInfoList(dev_info);
                dprintln!("HECI: No MEI device interfaces found");
                return Err(Error::NotFound);
            }

            // Get required size for detail data
            let mut required_size: u32 = 0;
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                ptr::null_mut(),
                0,
                &mut required_size,
                ptr::null_mut(),
            );

            if required_size == 0 {
                SetupDiDestroyDeviceInfoList(dev_info);
                return Err(Error::DeviceError);
            }

            // Allocate buffer for detail data (variable-length struct)
            let mut detail_buf = vec![0u8; required_size as usize];
            let detail = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            // cbSize must be size of the fixed part of the struct (on 64-bit: 8)
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

            let ok = SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface_data,
                detail,
                required_size,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            SetupDiDestroyDeviceInfoList(dev_info);

            if ok == 0 {
                dprintln!("HECI: GetDeviceInterfaceDetail failed");
                return Err(Error::DeviceError);
            }

            // DevicePath starts at offset of DevicePath field
            let path_ptr = &(*detail).DevicePath as *const u16;

            let handle = CreateFileW(
                path_ptr,
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                0 as HANDLE,
            );

            if handle == INVALID_HANDLE_VALUE {
                let err = GetLastError();
                dprintln!("HECI: CreateFile failed (error {})", err);
                return Err(Error::DeviceError);
            }

            self.handle = handle;
            dprintln!("HECI: Opened MEI device");
            Ok(())
        }
    }

    fn do_ioctl(&self, code: u32, in_buf: &[u8], out_buf: &mut [u8]) -> Result<u32> {
        unsafe {
            let event = CreateEventW(ptr::null(), 0, 0, ptr::null());
            if event == 0 as HANDLE {
                return Err(Error::DeviceError);
            }

            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = event;

            let mut bytes_returned: u32 = 0;

            let ok = DeviceIoControl(
                self.handle,
                code,
                in_buf.as_ptr() as *const _,
                in_buf.len() as u32,
                out_buf.as_mut_ptr() as *mut _,
                out_buf.len() as u32,
                &mut bytes_returned,
                &mut overlapped,
            );

            if ok == 0 && GetLastError() == ERROR_IO_PENDING {
                WaitForSingleObject(event, INFINITE);
                GetOverlappedResult(self.handle, &overlapped, &mut bytes_returned, 1);
            } else if ok == 0 {
                let err = GetLastError();
                CloseHandle(event);
                dprintln!("HECI: IOCTL failed (error {})", err);
                return Err(Error::DeviceError);
            }

            CloseHandle(event);
            Ok(bytes_returned)
        }
    }

    fn connect_client_by_uuid(&mut self, uuid: &[u8; 16]) -> Result<()> {
        let ioctl_connect = ctl_code(FILE_DEVICE_HECI, 0x801, 0, FILE_SHARE_READ | FILE_SHARE_WRITE);
        let mut out_buf = [0u8; 16];

        self.do_ioctl(ioctl_connect, uuid, &mut out_buf)?;

        // Response: MaxMessageLength (u32 LE) + ProtocolVersion (u8) + Reserved (3)
        self.max_msg_len = u32::from_le_bytes([out_buf[0], out_buf[1], out_buf[2], out_buf[3]]);
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
        unsafe {
            let event = CreateEventW(ptr::null(), 0, 0, ptr::null());
            if event == 0 as HANDLE {
                return Err(Error::DeviceError);
            }

            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = event;

            let mut bytes_written: u32 = 0;

            let ok = WriteFile(
                self.handle,
                data.as_ptr(),
                data.len() as u32,
                &mut bytes_written,
                &mut overlapped,
            );

            if ok == 0 && GetLastError() == ERROR_IO_PENDING {
                let wait = WaitForSingleObject(event, 5000);
                if wait != WAIT_OBJECT_0 {
                    CloseHandle(event);
                    return Err(Error::Timeout);
                }
                GetOverlappedResult(self.handle, &overlapped, &mut bytes_written, 0);
            } else if ok == 0 {
                CloseHandle(event);
                return Err(Error::DeviceError);
            }

            CloseHandle(event);
            dprintln!("  [heci_send] Sent {} bytes", bytes_written);
            Ok(())
        }
    }

    /// Receive data from the connected ME client.
    /// Uses an internal buffer sized to max_msg_len (MEI driver requires this).
    pub fn receive(&self, buf: &mut [u8]) -> Result<u32> {
        // MEI driver needs a read buffer >= max_msg_len, not the caller's small buffer
        let read_size = self.max_msg_len.max(4096) as usize;
        let mut tmp_buf = vec![0u8; read_size];

        unsafe {
            let event = CreateEventW(ptr::null(), 0, 0, ptr::null());
            if event == 0 as HANDLE {
                return Err(Error::DeviceError);
            }

            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = event;

            let mut bytes_read: u32 = 0;

            let ok = ReadFile(
                self.handle,
                tmp_buf.as_mut_ptr(),
                read_size as u32,
                &mut bytes_read,
                &mut overlapped,
            );

            if ok == 0 && GetLastError() == ERROR_IO_PENDING {
                let wait = WaitForSingleObject(event, 10000);
                if wait != WAIT_OBJECT_0 {
                    CloseHandle(event);
                    return Err(Error::Timeout);
                }
                GetOverlappedResult(self.handle, &overlapped, &mut bytes_read, 0);
            } else if ok == 0 {
                CloseHandle(event);
                return Err(Error::DeviceError);
            }

            CloseHandle(event);
            dprintln!("  [heci_recv] Got {} bytes", bytes_read);

            // Copy to caller's buffer
            let copy_len = (bytes_read as usize).min(buf.len());
            buf[..copy_len].copy_from_slice(&tmp_buf[..copy_len]);
            Ok(bytes_read)
        }
    }

    pub fn close(&mut self) {
        dprintln!("HECI: Closing");
        // Intentionally do NOT call CloseHandle — Intel MEI driver bug
        // (null-pointer AV in kernel close dispatch on some driver versions)
        self.handle = INVALID_HANDLE_VALUE;
        self.max_msg_len = 0;
    }
}
