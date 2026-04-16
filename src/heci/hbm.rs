// HBM (Host Bus Message) command codes
pub const HBM_CMD_HOST_VERSION: u8 = 0x01;
pub const HBM_CMD_HOST_VERSION_RESP: u8 = 0x81;
pub const HBM_CMD_HOST_ENUM: u8 = 0x04;
pub const HBM_CMD_HOST_ENUM_RESP: u8 = 0x84;
pub const HBM_CMD_HOST_CLIENT_PROP: u8 = 0x05;
pub const HBM_CMD_HOST_CLIENT_PROP_RESP: u8 = 0x85;
pub const HBM_CMD_CONNECT: u8 = 0x06;
pub const HBM_CMD_CONNECT_RESP: u8 = 0x86;
pub const HBM_CMD_FLOW_CONTROL: u8 = 0x08;

pub const HBM_MAJOR_VERSION: u8 = 2;
pub const HBM_MINOR_VERSION: u8 = 0;

// HBM Client Disconnect Request/Response
pub const HBM_CMD_CLIENT_DISCONNECT_REQ: u8 = 0x07;
pub const HBM_CMD_CLIENT_DISCONNECT_RESP: u8 = 0x87;

// AMTHI client UUID: {12F80028-B4B7-4B2D-ACA8-46E0FF65814C}
pub const AMTHI_UUID: [u8; 16] = [
    0x28, 0x00, 0xf8, 0x12, 0xb7, 0xb4, 0x2d, 0x4b,
    0xac, 0xa8, 0x46, 0xe0, 0xff, 0x65, 0x81, 0x4c,
];

#[repr(C, packed)]
pub struct HbmFlowControl {
    pub cmd: u8,
    pub me_addr: u8,
    pub host_addr: u8,
    pub reserved: [u8; 5],
}

#[repr(C, packed)]
pub struct HbmHostVersionReq {
    pub cmd: u8,
    pub minor: u8,
    pub major: u8,
    pub reserved: u8,
}

#[repr(C, packed)]
pub struct HbmHostVersionResp {
    pub cmd: u8,
    pub minor: u8,
    pub major: u8,
    pub supported: u8,
}

#[repr(C, packed)]
pub struct HbmHostEnumReq {
    pub cmd: u8,
    pub reserved: [u8; 3],
}

#[repr(C, packed)]
pub struct HbmHostEnumResp {
    pub cmd: u8,
    pub reserved: [u8; 3],
    pub valid_addresses: [u8; 32],
}

#[repr(C, packed)]
pub struct HbmClientPropReq {
    pub cmd: u8,
    pub me_addr: u8,
    pub reserved: [u8; 2],
}

#[repr(C, packed)]
pub struct HbmClientPropResp {
    pub cmd: u8,
    pub me_addr: u8,
    pub status: u8,
    pub reserved: u8,
    pub uuid: [u8; 16],
    pub protocol_ver: u8,
    pub max_connections: u8,
    pub fixed_address: u8,
    pub single_recv_buf: u8,
    pub max_msg_length: u32,
}

#[repr(C, packed)]
pub struct HbmConnectReq {
    pub cmd: u8,
    pub me_addr: u8,
    pub host_addr: u8,
    pub reserved: u8,
}

#[repr(C, packed)]
pub struct HbmConnectResp {
    pub cmd: u8,
    pub me_addr: u8,
    pub host_addr: u8,
    pub status: u8,
}
