#include "amt.h"

/* ----------------------------------------------------------------
 * pthi_make_header: build a PTHI request header
 * ---------------------------------------------------------------- */
PTHI_MSG_HEADER pthi_make_header(UINT32 command, UINT32 payload_length)
{
    PTHI_MSG_HEADER hdr;
    hdr.version.major = PTHI_MAJOR_VERSION;
    hdr.version.minor = PTHI_MINOR_VERSION;
    hdr.reserved = 0;
    hdr.command = command;
    hdr.length = payload_length;
    return hdr;
}

/* ----------------------------------------------------------------
 * pthi_call: send request, receive response, check status
 * ---------------------------------------------------------------- */
EFI_STATUS pthi_call(HECI_CONTEXT *ctx,
                     const void *cmd, UINT32 cmd_len,
                     void *resp, UINT32 resp_size, UINT32 *resp_len)
{
    EFI_STATUS status;
    PTHI_MSG_HEADER *req_hdr = (PTHI_MSG_HEADER *)cmd;

    Print(L"\r\n--- PTHI Call: cmd=0x%08x len=%d ---\r\n",
          req_hdr->command, cmd_len);

    /* Brief delay before each PTHI exchange to let ME settle */
    uefi_call_wrapper(ctx->ST->BootServices->Stall, 1, 10000); /* 10ms */

    status = heci_send(ctx, cmd, cmd_len);
    if (EFI_ERROR(status)) {
        Print(L"PTHI: Send failed (0x%lx)\r\n", status);
        return status;
    }

    UINT32 received = 0;
    status = heci_receive(ctx, resp, resp_size, &received);
    if (EFI_ERROR(status)) {
        Print(L"PTHI: Receive failed (0x%lx)\r\n", status);
        return status;
    }

    Print(L"PTHI: Got %d bytes back\r\n", received);

    if (resp_len)
        *resp_len = received;

    /* Check PTHI status in response header */
    if (received >= PTHI_RESP_HEADER_SIZE) {
        PTHI_RESP_HEADER *rh = (PTHI_RESP_HEADER *)resp;
        Print(L"PTHI: Response cmd=0x%08x status=%d\r\n",
              rh->header.command, rh->status);
        if (rh->status != AMT_STATUS_SUCCESS) {
            Print(L"PTHI: Command 0x%08x returned AMT error %d\r\n",
                  rh->header.command, rh->status);
            return EFI_DEVICE_ERROR;
        }
    }

    Print(L"--- PTHI Call OK ---\r\n");

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * amt_get_control_mode
 * ---------------------------------------------------------------- */
EFI_STATUS amt_get_control_mode(HECI_CONTEXT *ctx, UINT32 *mode)
{
    Print(L"AMT: GetControlMode\r\n");
    PTHI_MSG_HEADER req = pthi_make_header(PTHI_GET_CONTROL_MODE_REQUEST, 0);
    PTHI_GET_CONTROL_MODE_RESP resp;
    UINT32 resp_len;

    EFI_STATUS status = pthi_call(ctx,
                                  &req, sizeof(req),
                                  &resp, sizeof(resp), &resp_len);
    if (EFI_ERROR(status))
        return status;

    Print(L"AMT: ControlMode raw value = %d\r\n", resp.state);
    *mode = resp.state;
    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * amt_get_provisioning_state
 * ---------------------------------------------------------------- */
EFI_STATUS amt_get_provisioning_state(HECI_CONTEXT *ctx, UINT32 *state)
{
    Print(L"AMT: GetProvisioningState\r\n");
    PTHI_MSG_HEADER req = pthi_make_header(PTHI_GET_PROVISIONING_STATE_REQUEST, 0);
    PTHI_GET_PROVISIONING_STATE_RESP resp;
    UINT32 resp_len;

    EFI_STATUS status = pthi_call(ctx,
                                  &req, sizeof(req),
                                  &resp, sizeof(resp), &resp_len);
    if (EFI_ERROR(status))
        return status;

    Print(L"AMT: ProvisioningState raw value = %d\r\n", resp.state);
    *state = resp.state;
    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * amt_get_uuid
 * ---------------------------------------------------------------- */
EFI_STATUS amt_get_uuid(HECI_CONTEXT *ctx, UINT8 *uuid_out)
{
    Print(L"AMT: GetUUID\r\n");
    PTHI_MSG_HEADER req = pthi_make_header(PTHI_GET_UUID_REQUEST, 0);
    PTHI_GET_UUID_RESP resp;
    UINT32 resp_len;

    EFI_STATUS status = pthi_call(ctx,
                                  &req, sizeof(req),
                                  &resp, sizeof(resp), &resp_len);
    if (EFI_ERROR(status))
        return status;

    for (int i = 0; i < 16; i++)
        uuid_out[i] = resp.uuid[i];

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * amt_get_local_system_account
 *
 * Request is special: header with length=40 + 40 reserved bytes = 52 total.
 * This matches RPC-Go's GetLocalSystemAccount exactly.
 * ---------------------------------------------------------------- */
EFI_STATUS amt_get_local_system_account(HECI_CONTEXT *ctx,
                                        AMT_LSA_CREDENTIALS *creds)
{
    Print(L"AMT: GetLocalSystemAccount\r\n");

    PTHI_GET_LSA_REQUEST req;
    /*
     * Use an oversized raw buffer — the actual response may have
     * padding beyond the documented 82 bytes (observed: 84 bytes).
     */
    UINT8 resp_buf[256];
    UINT32 resp_len;

    /* Build request: header with length=40, then 40 zero bytes */
    req.header = pthi_make_header(PTHI_GET_LOCAL_SYSTEM_ACCOUNT_REQUEST, 40);
    for (int i = 0; i < 40; i++)
        req.reserved[i] = 0;

    Print(L"AMT: LSA request size=%d (header.length=%d)\r\n",
          (UINT32)sizeof(req), req.header.length);

    EFI_STATUS status = pthi_call(ctx,
                                  &req, sizeof(req),
                                  resp_buf, sizeof(resp_buf), &resp_len);
    if (EFI_ERROR(status))
        return status;

    Print(L"AMT: LSA response total=%d bytes\r\n", resp_len);

    /*
     * Parse response manually:
     *   Offset 0:  PTHI response header (16 bytes)
     *   Offset 16: username (33 bytes, null-terminated)
     *   Offset 49: password (33 bytes, null-terminated)
     */
    if (resp_len < PTHI_RESP_HEADER_SIZE + LSA_USERNAME_LEN + LSA_PASSWORD_LEN) {
        Print(L"AMT: LSA response too short (%d bytes)\r\n", resp_len);
        return EFI_DEVICE_ERROR;
    }

    UINT8 *data = resp_buf + PTHI_RESP_HEADER_SIZE;

    for (int i = 0; i < LSA_USERNAME_LEN; i++)
        creds->username[i] = (CHAR8)data[i];
    creds->username[LSA_USERNAME_LEN - 1] = '\0';

    for (int i = 0; i < LSA_PASSWORD_LEN; i++)
        creds->password[i] = (CHAR8)data[LSA_USERNAME_LEN + i];
    creds->password[LSA_PASSWORD_LEN - 1] = '\0';

    /* Log username, only password length (not the actual password) */
    int pwd_len = 0;
    while (pwd_len < LSA_PASSWORD_LEN && creds->password[pwd_len])
        pwd_len++;

    Print(L"AMT: LSA username='%a' password_len=%d\r\n",
          creds->username, pwd_len);

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * amt_control_mode_str
 * ---------------------------------------------------------------- */
const CHAR16 *amt_control_mode_str(UINT32 mode)
{
    switch (mode) {
    case AMT_CONTROL_MODE_PRE_PROVISIONING:
        return L"Pre-Provisioning";
    case AMT_CONTROL_MODE_CCM:
        return L"Client Control Mode (CCM)";
    case AMT_CONTROL_MODE_ACM:
        return L"Admin Control Mode (ACM)";
    default:
        return L"Unknown";
    }
}
