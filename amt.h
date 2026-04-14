#ifndef AMT_H
#define AMT_H

#include "heci.h"

/*
 * AMT PTHI (Power Technology Host Interface) protocol.
 *
 * Binary command/response protocol over HECI to query and control
 * Intel AMT. Message format matches RPC-Go pkg/pthi exactly.
 */

/* ---------------- PTHI Message Header ---------------- */
/*
 * Every PTHI request/response starts with a 12-byte header:
 *   Version   (2 bytes: major=1, minor=1)
 *   Reserved  (2 bytes: 0)
 *   Command   (4 bytes: request=0x04XXXXXX, response=0x048XXXXX)
 *   Length    (4 bytes: payload length after header)
 */
typedef struct {
    UINT8  major;
    UINT8  minor;
} PTHI_VERSION;

typedef struct {
    PTHI_VERSION version;
    UINT16       reserved;
    UINT32       command;
    UINT32       length;
} PTHI_MSG_HEADER;

/* Response header includes status after the message header */
typedef struct {
    PTHI_MSG_HEADER header;
    UINT32          status;
} PTHI_RESP_HEADER;

/* Header + status = 16 bytes */
#define PTHI_HEADER_SIZE      12
#define PTHI_RESP_HEADER_SIZE 16

/* PTHI version (always 1.1 per RPC-Go) */
#define PTHI_MAJOR_VERSION  1
#define PTHI_MINOR_VERSION  1

/* ---------------- Command IDs ---------------- */
/* From RPC-Go pkg/pthi/types.go — request and response pairs */

#define PTHI_GET_CONTROL_MODE_REQUEST   0x0400006B
#define PTHI_GET_CONTROL_MODE_RESPONSE  0x0480006B

#define PTHI_GET_PROVISIONING_STATE_REQUEST   0x04000011
#define PTHI_GET_PROVISIONING_STATE_RESPONSE  0x04800011

#define PTHI_GET_CODE_VERSIONS_REQUEST  0x0400001A
#define PTHI_GET_CODE_VERSIONS_RESPONSE 0x0480001A

#define PTHI_GET_UUID_REQUEST           0x0400005C
#define PTHI_GET_UUID_RESPONSE          0x0480005C

#define PTHI_GET_LOCAL_SYSTEM_ACCOUNT_REQUEST  0x04000067
#define PTHI_GET_LOCAL_SYSTEM_ACCOUNT_RESPONSE 0x04800067

/* ---------------- PTHI Status Codes ---------------- */
#define AMT_STATUS_SUCCESS          0
#define AMT_STATUS_INTERNAL_ERROR   1
#define AMT_STATUS_NOT_READY        2
#define AMT_STATUS_NOT_PERMITTED    16

/* ---------------- Control Mode Values ---------------- */
#define AMT_CONTROL_MODE_PRE_PROVISIONING  0
#define AMT_CONTROL_MODE_CCM              1
#define AMT_CONTROL_MODE_ACM              2

/* ---------------- Response Structures ---------------- */

/* GetControlMode response: header(16) + state(4) = 20 bytes */
typedef struct {
    PTHI_RESP_HEADER resp;
    UINT32           state;    /* 0=PreProv, 1=CCM, 2=ACM */
} PTHI_GET_CONTROL_MODE_RESP;

/* GetProvisioningState response */
typedef struct {
    PTHI_RESP_HEADER resp;
    UINT32           state;
} PTHI_GET_PROVISIONING_STATE_RESP;

/* GetUUID response */
typedef struct {
    PTHI_RESP_HEADER resp;
    UINT8            uuid[16];
} PTHI_GET_UUID_RESP;

/*
 * GetLocalSystemAccount
 *
 * Request:  PTHI header (length=40) + 40 bytes reserved = 52 bytes total
 * Response: PTHI resp header + username[33] + password[33] = 82 bytes
 *
 * From RPC-Go: CFG_MAX_ACL_USER_LENGTH = CFG_MAX_ACL_PWD_LENGTH = 33
 */
#define LSA_USERNAME_LEN  33
#define LSA_PASSWORD_LEN  33

typedef struct __attribute__((packed)) {
    PTHI_MSG_HEADER header;
    UINT8           reserved[40];
} PTHI_GET_LSA_REQUEST;

typedef struct __attribute__((packed)) {
    PTHI_RESP_HEADER resp;
    UINT8            username[LSA_USERNAME_LEN];
    UINT8            password[LSA_PASSWORD_LEN];
} PTHI_GET_LSA_RESPONSE;

/* Parsed LSA credentials (null-terminated strings) */
typedef struct {
    CHAR8  username[LSA_USERNAME_LEN];
    CHAR8  password[LSA_PASSWORD_LEN];
} AMT_LSA_CREDENTIALS;

/* ---------------- API ---------------- */

/*
 * Build a PTHI request header. For simple "get" commands,
 * the entire request IS just the header (length=0).
 */
PTHI_MSG_HEADER pthi_make_header(UINT32 command, UINT32 payload_length);

/*
 * Send a PTHI command and receive the response.
 * cmd/cmd_len: the request bytes (header + optional payload).
 * resp/resp_size/resp_len: buffer for the full response.
 * Returns EFI_SUCCESS if the PTHI status in the response is SUCCESS.
 */
EFI_STATUS pthi_call(HECI_CONTEXT *ctx,
                     const void *cmd, UINT32 cmd_len,
                     void *resp, UINT32 resp_size, UINT32 *resp_len);

/* High-level commands */
EFI_STATUS amt_get_control_mode(HECI_CONTEXT *ctx, UINT32 *mode);
EFI_STATUS amt_get_provisioning_state(HECI_CONTEXT *ctx, UINT32 *state);
EFI_STATUS amt_get_uuid(HECI_CONTEXT *ctx, UINT8 *uuid_out);
EFI_STATUS amt_get_local_system_account(HECI_CONTEXT *ctx,
                                        AMT_LSA_CREDENTIALS *creds);

/* Convert control mode to string */
const CHAR16 *amt_control_mode_str(UINT32 mode);

#endif /* AMT_H */
