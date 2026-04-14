#ifndef LME_H
#define LME_H

#include "heci.h"

/*
 * LME (Local Manageability Engine) / APF (AMT Port Forwarding) protocol.
 *
 * LME is a HECI client that provides TCP-like tunneling to AMT's
 * internal HTTP server (port 16992). Data flows over APF protocol
 * messages, which are all big-endian on the wire.
 *
 * Reference: go-wsman-messages/pkg/apf/ and rpc-go/internal/lm/
 */

/* ---- LME client GUID ---- */
/* {6733A4DB-0476-4E7B-B3AF-BCFC29BEE7A7} */
#define LME_UUID_BYTES \
    { 0xdb, 0xa4, 0x33, 0x67, 0x76, 0x04, 0x7b, 0x4e, \
      0xb3, 0xaf, 0xbc, 0xfc, 0x29, 0xbe, 0xe7, 0xa7 }

/* ---- APF message type codes ---- */
#define APF_DISCONNECT                1
#define APF_SERVICE_REQUEST           5
#define APF_SERVICE_ACCEPT            6
#define APF_USERAUTH_REQUEST          50
#define APF_USERAUTH_FAILURE          51
#define APF_USERAUTH_SUCCESS          52
#define APF_GLOBAL_REQUEST            80
#define APF_REQUEST_SUCCESS           81
#define APF_REQUEST_FAILURE           82
#define APF_CHANNEL_OPEN              90
#define APF_CHANNEL_OPEN_CONFIRMATION 91
#define APF_CHANNEL_OPEN_FAILURE      92
#define APF_CHANNEL_WINDOW_ADJUST     93
#define APF_CHANNEL_DATA              94
#define APF_CHANNEL_CLOSE             97
#define APF_PROTOCOLVERSION           192
#define APF_KEEPALIVE_REQUEST         208
#define APF_KEEPALIVE_REPLY           209

/* ---- APF constants ---- */
#define LME_RX_WINDOW_SIZE   4096
#define APF_AMT_HTTP_PORT    16992

/* ---- APF message structures (all big-endian on wire) ---- */

/* APF_PROTOCOL_VERSION (93 bytes) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;     /* 192 */
    UINT32 major_version;    /* BE: 1 */
    UINT32 minor_version;    /* BE: 0 */
    UINT32 trigger_reason;   /* BE: 9 */
    UINT8  uuid[16];
    UINT8  reserved[64];
} APF_PROTOCOL_VERSION_MSG;

/* APF_CHANNEL_OPEN (54 bytes) - for "forwarded-tcpip" */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 90 */
    UINT32 channel_type_length;       /* BE: 15 */
    UINT8  channel_type[15];          /* "forwarded-tcpip" */
    UINT32 sender_channel;            /* BE: our channel number */
    UINT32 initial_window_size;       /* BE: 4096 */
    UINT32 reserved;                  /* BE: 0xFFFFFFFF */
    UINT32 connected_addr_length;     /* BE: 3 */
    UINT8  connected_addr[3];         /* "::1" */
    UINT32 connected_port;            /* BE: 16992 */
    UINT32 originator_addr_length;    /* BE: 3 */
    UINT8  originator_addr[3];        /* "::1" */
    UINT32 originator_port;           /* BE: 123 */
} APF_CHANNEL_OPEN_MSG;

/* APF_CHANNEL_OPEN_CONFIRMATION (17 bytes) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 91 */
    UINT32 recipient_channel;         /* BE: echoes our sender_channel */
    UINT32 sender_channel;            /* BE: AMT's channel number */
    UINT32 initial_window_size;       /* BE: AMT's TX window */
    UINT32 reserved;                  /* 0xFFFFFFFF */
} APF_CHANNEL_OPEN_CONFIRM_MSG;

/* APF_CHANNEL_OPEN_FAILURE (17 bytes) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 92 */
    UINT32 recipient_channel;
    UINT32 reason_code;
    UINT32 reserved;
    UINT32 reserved2;
} APF_CHANNEL_OPEN_FAILURE_MSG;

/* APF_CHANNEL_DATA header (9 bytes + data) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 94 */
    UINT32 recipient_channel;         /* BE */
    UINT32 data_length;               /* BE */
    /* followed by data_length bytes of payload */
} APF_CHANNEL_DATA_HDR;

/* APF_CHANNEL_WINDOW_ADJUST (9 bytes) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 93 */
    UINT32 recipient_channel;         /* BE */
    UINT32 bytes_to_add;              /* BE */
} APF_CHANNEL_WINDOW_ADJUST_MSG;

/* APF_CHANNEL_CLOSE (5 bytes) */
typedef struct __attribute__((packed)) {
    UINT8  message_type;              /* 97 */
    UINT32 recipient_channel;         /* BE */
} APF_CHANNEL_CLOSE_MSG;

/* ---- LME session state ---- */
typedef struct {
    HECI_CONTEXT  heci;             /* HECI context (reused, reconnected to LME) */
    UINT32        our_channel;      /* Our channel number (sender_channel in OPEN) */
    UINT32        amt_channel;      /* AMT's channel number (from OPEN_CONFIRM) */
    UINT32        tx_window;        /* Remaining bytes we can send */
    UINT32        rx_window;        /* Remaining bytes AMT can send us */

    /* Receive buffer for accumulated APF data */
    UINT8         rx_buf[8192];
    UINT32        rx_len;

    EFI_SYSTEM_TABLE *ST;
} LME_SESSION;

/* ---- Big-endian helpers ---- */
static inline UINT32 be32(UINT32 v)
{
    return ((v & 0xFF) << 24) | ((v & 0xFF00) << 8) |
           ((v & 0xFF0000) >> 8) | ((v >> 24) & 0xFF);
}

static inline UINT32 read_be32(const UINT8 *p)
{
    return ((UINT32)p[0] << 24) | ((UINT32)p[1] << 16) |
           ((UINT32)p[2] << 8) | (UINT32)p[3];
}

/* ---- API ---- */

/*
 * Initialize LME: reset HECI, reconnect to LME client via HBM,
 * send APF_PROTOCOL_VERSION, process response.
 */
EFI_STATUS lme_init(LME_SESSION *lme, HECI_CONTEXT *old_heci,
                    EFI_SYSTEM_TABLE *SystemTable);

/*
 * Open an APF channel to AMT HTTP port (16992).
 * Sends CHANNEL_OPEN, processes responses until CHANNEL_OPEN_CONFIRMATION.
 */
EFI_STATUS lme_channel_open(LME_SESSION *lme);

/*
 * Send raw data through the APF channel (wraps in CHANNEL_DATA).
 */
EFI_STATUS lme_send(LME_SESSION *lme, const UINT8 *data, UINT32 len);

/*
 * Receive data from APF channel. Processes APF messages internally,
 * accumulates CHANNEL_DATA payloads into lme->rx_buf/rx_len.
 * Returns when data is available or timeout.
 */
EFI_STATUS lme_receive(LME_SESSION *lme, UINT32 timeout_ms);

/*
 * Close the APF channel.
 */
EFI_STATUS lme_close(LME_SESSION *lme);

#endif /* LME_H */
