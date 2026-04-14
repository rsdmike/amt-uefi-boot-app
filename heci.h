#ifndef HECI_H
#define HECI_H

#include <efi.h>
#include <efilib.h>

/*
 * HECI (Host Embedded Controller Interface) driver for UEFI.
 *
 * Communicates with the Intel Management Engine via PCI device 00:16.0.
 * Uses MMIO registers at BAR0 for circular buffer send/receive.
 *
 * Reference: Intel ME host interface specification, Linux mei driver,
 *            and RPC-Go pkg/heci implementation.
 */

/* ---------------- PCI location ---------------- */
#define HECI_BUS      0
#define HECI_DEVICE   22   /* 0x16 */
#define HECI_FUNCTION 0

/* PCI config space offsets */
#define PCI_VENDOR_ID     0x00
#define PCI_COMMAND       0x04
#define PCI_BAR0          0x10

#define PCI_CMD_MSE       (1 << 1)  /* Memory Space Enable */
#define PCI_CMD_BME       (1 << 2)  /* Bus Master Enable */

#define INTEL_VENDOR_ID   0x8086

/* ---------------- HECI MMIO registers (offsets from BAR0) ---------------- */
/*
 * The HECI MMIO register layout:
 *   0x00  H_CB_WW   - Host Circular Buffer Write Window
 *   0x04  H_CSR     - Host Control/Status Register
 *   0x08  ME_CB_RW  - ME Circular Buffer Read Window
 *   0x0C  ME_CSR_HA - ME Control/Status Register (Host Access)
 */
#define H_CB_WW    0x00
#define H_CSR      0x04
#define ME_CB_RW   0x08
#define ME_CSR_HA  0x0C

/* CSR (Control/Status Register) bit fields */
#define CSR_IE     (1 << 0)   /* Interrupt Enable */
#define CSR_IS     (1 << 1)   /* Interrupt Status */
#define CSR_IG     (1 << 2)   /* Interrupt Generate */
#define CSR_RDY    (1 << 3)   /* Ready */
#define CSR_RST    (1 << 4)   /* Reset */

/* CSR field positions */
#define CSR_RP_SHIFT  8     /* Read Pointer  [15:8]  */
#define CSR_WP_SHIFT  16    /* Write Pointer [23:16] */
#define CSR_CBD_SHIFT 24    /* Buffer Depth  [31:24] */

#define CSR_RP_MASK   0x0000FF00
#define CSR_WP_MASK   0x00FF0000
#define CSR_CBD_MASK  0xFF000000

/* Extract fields from CSR */
#define CSR_GET_RP(csr)   (((csr) & CSR_RP_MASK)  >> CSR_RP_SHIFT)
#define CSR_GET_WP(csr)   (((csr) & CSR_WP_MASK)  >> CSR_WP_SHIFT)
#define CSR_GET_CBD(csr)  (((csr) & CSR_CBD_MASK) >> CSR_CBD_SHIFT)

/* Compute number of filled slots in the circular buffer */
#define FILLED_SLOTS(wp, rp, depth) (((wp) - (rp)) % (depth))

/* ---------------- HECI message header ---------------- */
/*
 * Every HECI message starts with a 32-bit header:
 *   bits [7:0]   - ME address (client ID on ME side)
 *   bits [15:8]  - Host address
 *   bits [24:16] - Length (bytes of payload following this header)
 *   bits [30:25] - Reserved
 *   bit  [31]    - Message Complete flag
 */
typedef struct {
    UINT32 me_addr     : 8;
    UINT32 host_addr   : 8;
    UINT32 length      : 9;
    UINT32 reserved    : 6;
    UINT32 msg_complete: 1;
} HECI_MSG_HDR;

/* Max HECI payload in a single message (limited by CB depth) */
#define HECI_MAX_PAYLOAD  (4096)

/* Timeout for HECI operations (in microseconds) */
#define HECI_TIMEOUT_US   (5 * 1000 * 1000)  /* 5 seconds */
#define HECI_POLL_US      (1000)              /* 1ms poll interval */

/* ---------------- HBM (Host Bus Message) commands ---------------- */
/*
 * HBM is used on me_addr=0, host_addr=0 to enumerate and connect
 * to ME clients. For PTHI we need to find the AMTHI client
 * and establish a connection to get assigned addresses.
 */
#define HBM_CMD_HOST_VERSION       0x01
#define HBM_CMD_HOST_VERSION_RESP  0x81
#define HBM_CMD_HOST_ENUM          0x04
#define HBM_CMD_HOST_ENUM_RESP     0x84
#define HBM_CMD_HOST_CLIENT_PROP   0x05
#define HBM_CMD_HOST_CLIENT_PROP_RESP 0x85
#define HBM_CMD_CONNECT            0x06
#define HBM_CMD_CONNECT_RESP       0x86
#define HBM_CMD_FLOW_CONTROL       0x08
#define HBM_CMD_FLOW_CONTROL_RESP  0x88

#define HBM_MAJOR_VERSION  2
#define HBM_MINOR_VERSION  0

/* HBM Flow Control - sent in both directions to grant message credits */
typedef struct __attribute__((packed)) {
    UINT8  cmd;        /* 0x08 */
    UINT8  me_addr;    /* ME client this credit is for */
    UINT8  host_addr;  /* Host client this credit is for */
    UINT8  reserved[5];
} HBM_FLOW_CONTROL;

/* HBM Host Version Request */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  minor;
    UINT8  major;
    UINT8  reserved;
} HBM_HOST_VERSION_REQ;

/* HBM Host Version Response */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  minor;
    UINT8  major;
    UINT8  supported;  /* 1 = version supported */
} HBM_HOST_VERSION_RESP;

/* HBM Host Enumeration Request */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  reserved[3];
} HBM_HOST_ENUM_REQ;

/* HBM Host Enumeration Response - bitmap of valid ME client addresses */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  reserved[3];
    UINT8  valid_addresses[32];  /* 256 bits = 256 possible clients */
} HBM_HOST_ENUM_RESP;

/* HBM Client Properties Request */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  me_addr;
    UINT8  reserved[2];
} HBM_CLIENT_PROP_REQ;

/*
 * ME client GUID - stored as raw 16 bytes on the wire.
 * Matches Linux kernel uuid_le layout.
 */
typedef struct __attribute__((packed)) {
    UINT8 b[16];
} ME_CLIENT_UUID;

/*
 * HBM Client Properties Response
 * Field order matches Linux kernel mei_client_properties + hbm_props_response.
 */
typedef struct __attribute__((packed)) {
    UINT8          cmd;
    UINT8          me_addr;
    UINT8          status;          /* 0 = success */
    UINT8          reserved;
    ME_CLIENT_UUID uuid;            /* 16-byte client GUID */
    UINT8          protocol_ver;
    UINT8          max_connections;
    UINT8          fixed_address;
    UINT8          single_recv_buf;
    UINT32         max_msg_length;
} HBM_CLIENT_PROP_RESP;

/* HBM Connect Request */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  me_addr;
    UINT8  host_addr;
    UINT8  reserved;
} HBM_CONNECT_REQ;

/* HBM Connect Response */
typedef struct __attribute__((packed)) {
    UINT8  cmd;
    UINT8  me_addr;
    UINT8  host_addr;
    UINT8  status;     /* 0 = success */
} HBM_CONNECT_RESP;

/* ---------------- AMTHI (PTHI) client GUID ---------------- */
/*
 * GUID: {12F80028-B4B7-4B2D-ACA8-46E0FF65814C}
 * Raw bytes as used by Linux MEI driver (MEI_IAMTHIF):
 */
#define AMTHI_UUID_BYTES \
    { 0x28, 0x00, 0xf8, 0x12, 0xb7, 0xb4, 0x2d, 0x4b, \
      0xac, 0xa8, 0x46, 0xe0, 0xff, 0x65, 0x81, 0x4c }

/* ---------------- HECI context ---------------- */
typedef struct {
    volatile UINT32 *mmio;          /* BAR0 mapped address */
    UINT8            me_addr;       /* ME client address for connected client */
    UINT8            host_addr;     /* Host address assigned during connect */
    UINT32           max_msg_len;   /* Negotiated max message length */
    UINT8            cb_depth;      /* Circular buffer depth (slots) */
    EFI_SYSTEM_TABLE *ST;
} HECI_CONTEXT;

/* ---------------- API ---------------- */

/* Initialize HECI: find PCI device, map BAR0, reset interface */
EFI_STATUS heci_init(HECI_CONTEXT *ctx, EFI_SYSTEM_TABLE *SystemTable);

/* HBM: version exchange, enumerate clients, find AMTHI, connect */
EFI_STATUS heci_connect_amthi(HECI_CONTEXT *ctx);

/* HBM: connect to any ME client by raw UUID bytes (16-byte array) */
EFI_STATUS heci_connect_client(HECI_CONTEXT *ctx, const UINT8 target_uuid[16]);

/* Send a raw payload to the connected ME client */
EFI_STATUS heci_send(HECI_CONTEXT *ctx, const void *data, UINT32 len);

/* Receive a raw payload from the connected ME client */
EFI_STATUS heci_receive(HECI_CONTEXT *ctx, void *buf, UINT32 buf_size,
                        UINT32 *out_len);

/* Clean up */
void heci_close(HECI_CONTEXT *ctx);

#endif /* HECI_H */
