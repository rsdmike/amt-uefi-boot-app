#[derive(Debug, Clone, Copy)]
pub enum Error {
    Timeout,
    NotFound,
    DeviceError,
    BufferTooSmall,
    Unsupported,
    AccessDenied,
    ProtocolError,
    Aborted,
    AmtStatus(u32),
    HttpStatus(u32),
}

#[cfg(feature = "uefi-target")]
impl From<Error> for uefi::Status {
    fn from(e: Error) -> uefi::Status {
        match e {
            Error::Timeout => uefi::Status::TIMEOUT,
            Error::NotFound => uefi::Status::NOT_FOUND,
            Error::DeviceError => uefi::Status::DEVICE_ERROR,
            Error::BufferTooSmall => uefi::Status::BUFFER_TOO_SMALL,
            Error::Unsupported => uefi::Status::UNSUPPORTED,
            Error::AccessDenied => uefi::Status::ACCESS_DENIED,
            Error::ProtocolError => uefi::Status::PROTOCOL_ERROR,
            Error::Aborted => uefi::Status::ABORTED,
            Error::AmtStatus(_) => uefi::Status::DEVICE_ERROR,
            Error::HttpStatus(_) => uefi::Status::DEVICE_ERROR,
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
