// APF message type codes
// pub const APF_DISCONNECT: u8 = 1;
pub const APF_SERVICE_REQUEST: u8 = 5;
pub const APF_SERVICE_ACCEPT: u8 = 6;
pub const APF_GLOBAL_REQUEST: u8 = 80;
pub const APF_REQUEST_SUCCESS: u8 = 81;
pub const APF_CHANNEL_OPEN: u8 = 90;
pub const APF_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
pub const APF_CHANNEL_OPEN_FAILURE: u8 = 92;
pub const APF_CHANNEL_WINDOW_ADJUST: u8 = 93;
pub const APF_CHANNEL_DATA: u8 = 94;
pub const APF_CHANNEL_CLOSE: u8 = 97;
pub const APF_PROTOCOLVERSION: u8 = 192;
pub const APF_KEEPALIVE_REQUEST: u8 = 208;
pub const APF_KEEPALIVE_REPLY: u8 = 209;

pub const LME_RX_WINDOW_SIZE: u32 = 4096;
pub const APF_AMT_HTTP_PORT: u32 = 16992;

// LME client UUID: {6733A4DB-0476-4E7B-B3AF-BCFC29BEE7A7}
pub const LME_UUID: [u8; 16] = [
    0xdb, 0xa4, 0x33, 0x67, 0x76, 0x04, 0x7b, 0x4e,
    0xb3, 0xaf, 0xbc, 0xfc, 0x29, 0xbe, 0xe7, 0xa7,
];

/// Read a big-endian u32 from a byte slice.
#[inline]
pub fn read_be32(p: &[u8]) -> u32 {
    u32::from_be_bytes([p[0], p[1], p[2], p[3]])
}

/// Write a big-endian u32 into a byte slice.
#[inline]
pub fn write_be32(buf: &mut [u8], val: u32) {
    let bytes = val.to_be_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    buf[2] = bytes[2];
    buf[3] = bytes[3];
}
