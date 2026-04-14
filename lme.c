#include "lme.h"

/* ----------------------------------------------------------------
 * Internal: raw HECI send/receive for LME
 *
 * LME uses a DIFFERENT flow than AMTHI. With AMTHI, we send HBM
 * flow control credits with every exchange. With LME, the APF
 * protocol handles its own flow control (window adjust). Sending
 * unsolicited HBM flow control credits confuses the ME and causes
 * it to disconnect us.
 *
 * So we use heci_write_msg/heci_read_msg directly, bypassing
 * the heci_send/heci_receive wrappers that add HBM flow control.
 * ---------------------------------------------------------------- */

/* Forward declarations for heci internal functions we need */
extern EFI_STATUS heci_write_msg(HECI_CONTEXT *ctx, UINT8 me_addr,
                                  UINT8 host_addr, const void *data,
                                  UINT32 len, BOOLEAN complete);
extern EFI_STATUS heci_read_msg(HECI_CONTEXT *ctx, void *buf,
                                 UINT32 buf_size, UINT32 *out_len,
                                 UINT8 *out_me_addr, UINT8 *out_host_addr);

/* Send HBM flow control credit — grants ME permission to send one message */
static EFI_STATUS lme_send_flow_control(LME_SESSION *lme)
{
    UINT8 fc[8] = {0};
    fc[0] = 0x08;  /* HBM_CMD_FLOW_CONTROL */
    fc[1] = lme->heci.me_addr;
    fc[2] = lme->heci.host_addr;
    return heci_write_msg(&lme->heci, 0, 0, fc, 8, TRUE);
}

/* Send raw data to LME — no automatic HBM flow control */
static EFI_STATUS lme_raw_send(LME_SESSION *lme, const void *data, UINT32 len)
{
    return heci_write_msg(&lme->heci, lme->heci.me_addr,
                          lme->heci.host_addr, data, len, TRUE);
}

/*
 * Receive from LME — skip HBM messages, return client messages.
 * No automatic HBM flow control credits sent.
 */
static EFI_STATUS lme_raw_recv(LME_SESSION *lme, UINT8 *buf,
                               UINT32 buf_size, UINT32 *out_len)
{
    UINT8 tmp_buf[4096];
    UINT32 tmp_len;
    UINT8 msg_me, msg_host;

    for (;;) {
        EFI_STATUS status = heci_read_msg(&lme->heci, tmp_buf, sizeof(tmp_buf),
                                          &tmp_len, &msg_me, &msg_host);
        if (EFI_ERROR(status))
            return status;

        /* Skip HBM messages (me=0, host=0) */
        if (msg_me == 0 && msg_host == 0) {
            UINT8 hbm_cmd = tmp_buf[0];
            if (hbm_cmd == 0x08) { /* flow control */
                Print(L"  [lme_recv] HBM flow control (ME:%d Host:%d)\r\n",
                      tmp_buf[1], tmp_buf[2]);
            } else if (hbm_cmd == 0x07 && tmp_len >= 4) { /* disconnect req */
                Print(L"  [lme_recv] HBM DISCONNECT REQ: me=%d host=%d status=%d\r\n",
                      tmp_buf[1], tmp_buf[2], tmp_buf[3]);
                /* Respond to disconnect */
                UINT8 resp[4] = { 0x87, tmp_buf[1], tmp_buf[2], 0 };
                heci_write_msg(&lme->heci, 0, 0, resp, 4, TRUE);
            } else {
                Print(L"  [lme_recv] HBM cmd=0x%02x len=%d\r\n", hbm_cmd, tmp_len);
            }
            continue;
        }

        /* Got a client message */
        if (msg_me == lme->heci.me_addr && msg_host == lme->heci.host_addr) {
            if (tmp_len > buf_size)
                return EFI_BUFFER_TOO_SMALL;
            for (UINT32 i = 0; i < tmp_len; i++)
                buf[i] = tmp_buf[i];
            if (out_len) *out_len = tmp_len;

            /* Grant ME another credit so it can send the next message */
            lme_send_flow_control(lme);

            return EFI_SUCCESS;
        }

        Print(L"  [lme_recv] Unexpected: me=%d host=%d len=%d\r\n",
              msg_me, msg_host, tmp_len);
    }
}

/* ----------------------------------------------------------------
 * Internal: process one APF message
 * ---------------------------------------------------------------- */
static UINT8 lme_process_apf(LME_SESSION *lme, const UINT8 *data, UINT32 len)
{
    if (len == 0) return 0;

    UINT8 msg_type = data[0];

    switch (msg_type) {
    case APF_PROTOCOLVERSION:
        if (len >= 93) {
            UINT32 major = read_be32(data + 1);
            UINT32 minor = read_be32(data + 5);
            Print(L"  [APF] Protocol version %d.%d\r\n", major, minor);
        }
        break;

    case APF_CHANNEL_OPEN_CONFIRMATION:
        if (len >= 17) {
            lme->our_channel = read_be32(data + 1);
            lme->amt_channel = read_be32(data + 5);
            lme->tx_window = read_be32(data + 9);
            Print(L"  [APF] Channel CONFIRMED: ours=%d amt=%d window=%d\r\n",
                  lme->our_channel, lme->amt_channel, lme->tx_window);
        }
        break;

    case APF_CHANNEL_OPEN_FAILURE:
        if (len >= 9) {
            UINT32 reason = read_be32(data + 5);
            Print(L"  [APF] Channel FAILED: reason=%d\r\n", reason);
        }
        break;

    case APF_CHANNEL_WINDOW_ADJUST:
        if (len >= 9) {
            UINT32 bytes_to_add = read_be32(data + 5);
            lme->tx_window += bytes_to_add;
            Print(L"  [APF] Window adjust +%d (now %d)\r\n",
                  bytes_to_add, lme->tx_window);
        }
        break;

    case APF_CHANNEL_DATA:
        if (len >= 9) {
            UINT32 data_len = read_be32(data + 5);
            const UINT8 *payload = data + 9;
            Print(L"  [APF] Channel data: %d bytes\r\n", data_len);
            if (data_len > 0 && (lme->rx_len + data_len) <= sizeof(lme->rx_buf)) {
                for (UINT32 i = 0; i < data_len; i++)
                    lme->rx_buf[lme->rx_len++] = payload[i];
            }
        }
        break;

    case APF_CHANNEL_CLOSE:
        Print(L"  [APF] Channel close\r\n");
        break;

    case APF_SERVICE_REQUEST: {
        /* Parse: [type(1)][name_len(4 BE)][name(name_len)] */
        if (len < 5) break;
        UINT32 name_len = read_be32(data + 1);
        if (len < 5 + name_len) break;
        Print(L"  [APF] Service request: '");
        for (UINT32 i = 0; i < name_len; i++)
            Print(L"%c", data[5 + i]);
        Print(L"'\r\n");

        /* Respond with APF_SERVICE_ACCEPT */
        UINT8 accept[64];
        UINT32 alen = 5 + name_len;
        accept[0] = APF_SERVICE_ACCEPT;
        accept[1] = (name_len >> 24) & 0xFF;
        accept[2] = (name_len >> 16) & 0xFF;
        accept[3] = (name_len >> 8) & 0xFF;
        accept[4] = name_len & 0xFF;
        for (UINT32 i = 0; i < name_len && i < 32; i++)
            accept[5 + i] = data[5 + i];
        lme_raw_send(lme, accept, alen);
        Print(L"  [APF] Sent service accept\r\n");
        break;
    }

    case APF_GLOBAL_REQUEST: {
        /*
         * Format: [type(1)][name_len(4 BE)][name][want_reply(1)]
         *   For "tcpip-forward": ...[addr_len(4 BE)][addr][port(4 BE)]
         */
        if (len < 5) break;
        UINT32 name_len = read_be32(data + 1);
        if (len < 6 + name_len) break;

        UINT8 want_reply = data[5 + name_len];
        UINT32 offset = 6 + name_len;

        /* Extract port from tcpip-forward request */
        UINT32 fwd_port = 0;
        if (offset + 4 <= len) {
            UINT32 addr_len = read_be32(data + offset);
            offset += 4 + addr_len;
            if (offset + 4 <= len)
                fwd_port = read_be32(data + offset);
        }

        Print(L"  [APF] Global request: port=%d want_reply=%d\r\n",
              fwd_port, want_reply);

        if (want_reply) {
            /* Respond with APF_REQUEST_SUCCESS + echoed port */
            UINT8 success[5];
            success[0] = APF_REQUEST_SUCCESS;
            success[1] = (fwd_port >> 24) & 0xFF;
            success[2] = (fwd_port >> 16) & 0xFF;
            success[3] = (fwd_port >> 8) & 0xFF;
            success[4] = fwd_port & 0xFF;
            lme_raw_send(lme, success, 5);
            Print(L"  [APF] Sent request success (port=%d)\r\n", fwd_port);
        }
        break;
    }

    case APF_KEEPALIVE_REQUEST:
        if (len >= 5) {
            UINT8 reply[5];
            reply[0] = APF_KEEPALIVE_REPLY;
            reply[1] = data[1]; reply[2] = data[2];
            reply[3] = data[3]; reply[4] = data[4];
            lme_raw_send(lme, reply, 5);
            Print(L"  [APF] Keepalive reply\r\n");
        }
        break;

    default:
        Print(L"  [APF] Unknown type=%d len=%d\r\n", msg_type, len);
        break;
    }

    return msg_type;
}

/* ----------------------------------------------------------------
 * lme_init: reconnect HECI to LME, APF version exchange
 * ---------------------------------------------------------------- */
EFI_STATUS lme_init(LME_SESSION *lme, HECI_CONTEXT *old_heci,
                    EFI_SYSTEM_TABLE *SystemTable)
{
    EFI_STATUS status;

    lme->ST = SystemTable;
    lme->our_channel = 0;
    lme->amt_channel = 0;
    lme->tx_window = 0;
    lme->rx_window = LME_RX_WINDOW_SIZE;
    lme->rx_len = 0;

    Print(L"LME: Closing AMTHI and resetting HECI...\r\n");
    heci_close(old_heci);

    status = heci_init(&lme->heci, SystemTable);
    if (EFI_ERROR(status)) {
        Print(L"LME: HECI re-init failed\r\n");
        return status;
    }

    Print(L"LME: Connecting to LME client via HBM...\r\n");
    {
        static const UINT8 lme_uuid[16] = LME_UUID_BYTES;
        status = heci_connect_client(&lme->heci, lme_uuid);
    }
    if (EFI_ERROR(status)) {
        Print(L"LME: Failed to connect to LME client\r\n");
        return status;
    }

    /* Grant ME a flow control credit so it can respond to us */
    Print(L"LME: Sending initial flow control credit...\r\n");
    lme_send_flow_control(lme);

    /* Send APF_PROTOCOL_VERSION (1, 0, 9) */
    Print(L"LME: Sending APF protocol version 1.0.9...\r\n");
    APF_PROTOCOL_VERSION_MSG pv;
    for (int i = 0; i < (int)sizeof(pv); i++)
        ((UINT8 *)&pv)[i] = 0;
    pv.message_type = APF_PROTOCOLVERSION;
    pv.major_version = be32(1);
    pv.minor_version = be32(0);
    pv.trigger_reason = be32(9);

    status = lme_raw_send(lme, &pv, sizeof(pv));
    if (EFI_ERROR(status)) {
        Print(L"LME: Failed to send protocol version\r\n");
        return status;
    }

    /*
     * Process APF init messages. AMT sends back:
     * 1. APF_PROTOCOL_VERSION (we must echo it back per RPC-Go flow)
     * 2. APF_SERVICE_REQUEST (we respond with SERVICE_ACCEPT)
     * 3. APF_GLOBAL_REQUEST (we respond with REQUEST_SUCCESS + port)
     * Loop until timeout (normal exit — means AMT is done sending).
     */
    Print(L"LME: Processing APF init...\r\n");
    UINT8 resp_buf[512];
    UINT32 resp_len;
    int protocol_version_ok = 0;

    for (int attempts = 0; attempts < 20; attempts++) {
        status = lme_raw_recv(lme, resp_buf, sizeof(resp_buf), &resp_len);
        if (EFI_ERROR(status)) {
            if (protocol_version_ok)
                break; /* normal: AMT done sending init messages */
            Print(L"LME: Init recv timeout (attempt %d)\r\n", attempts);
            continue;
        }

        UINT8 msg_type = lme_process_apf(lme, resp_buf, resp_len);

        if (msg_type == APF_PROTOCOLVERSION && !protocol_version_ok) {
            protocol_version_ok = 1;
            /* Echo protocol version back to AMT (required by APF flow) */
            Print(L"LME: Echoing protocol version...\r\n");
            APF_PROTOCOL_VERSION_MSG echo;
            for (int i = 0; i < (int)sizeof(echo); i++)
                ((UINT8 *)&echo)[i] = 0;
            echo.message_type = APF_PROTOCOLVERSION;
            echo.major_version = be32(read_be32(resp_buf + 1));
            echo.minor_version = be32(read_be32(resp_buf + 5));
            echo.trigger_reason = be32(read_be32(resp_buf + 9));
            lme_raw_send(lme, &echo, sizeof(echo));
        }
    }

    Print(L"LME: APF init complete (version_ok=%d)\r\n", protocol_version_ok);

    if (!protocol_version_ok)
        return EFI_UNSUPPORTED;

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * lme_channel_open: open APF channel to port 16992
 * ---------------------------------------------------------------- */
EFI_STATUS lme_channel_open(LME_SESSION *lme)
{
    EFI_STATUS status;

    lme->our_channel = 1;
    lme->amt_channel = 0;
    lme->tx_window = 0;

    APF_CHANNEL_OPEN_MSG msg;
    for (int i = 0; i < (int)sizeof(msg); i++)
        ((UINT8 *)&msg)[i] = 0;

    msg.message_type = APF_CHANNEL_OPEN;
    msg.channel_type_length = be32(15);
    const char *ct = "forwarded-tcpip";
    for (int i = 0; i < 15; i++)
        msg.channel_type[i] = ct[i];

    msg.sender_channel = be32(lme->our_channel);
    msg.initial_window_size = be32(LME_RX_WINDOW_SIZE);
    msg.reserved = be32(0xFFFFFFFF);
    msg.connected_addr_length = be32(3);
    msg.connected_addr[0] = ':'; msg.connected_addr[1] = ':';
    msg.connected_addr[2] = '1';
    msg.connected_port = be32(APF_AMT_HTTP_PORT);
    msg.originator_addr_length = be32(3);
    msg.originator_addr[0] = ':'; msg.originator_addr[1] = ':';
    msg.originator_addr[2] = '1';
    msg.originator_port = be32(123);

    Print(L"LME: Sending CHANNEL_OPEN to port %d...\r\n", APF_AMT_HTTP_PORT);

    status = lme_raw_send(lme, &msg, sizeof(msg));
    if (EFI_ERROR(status)) {
        Print(L"LME: CHANNEL_OPEN send failed\r\n");
        return status;
    }

    /* Wait for CHANNEL_OPEN_CONFIRMATION */
    UINT8 resp_buf[512];
    UINT32 resp_len;

    for (int attempts = 0; attempts < 30; attempts++) {
        status = lme_raw_recv(lme, resp_buf, sizeof(resp_buf), &resp_len);
        if (EFI_ERROR(status)) {
            Print(L"LME: Recv failed waiting for CHANNEL_OPEN_CONFIRM\r\n");
            return status;
        }

        UINT8 msg_type = lme_process_apf(lme, resp_buf, resp_len);

        if (msg_type == APF_CHANNEL_OPEN_CONFIRMATION) {
            Print(L"LME: Channel open! AMT channel=%d TX window=%d\r\n",
                  lme->amt_channel, lme->tx_window);
            return EFI_SUCCESS;
        }

        if (msg_type == APF_CHANNEL_OPEN_FAILURE)
            return EFI_DEVICE_ERROR;
    }

    Print(L"LME: Timeout waiting for CHANNEL_OPEN_CONFIRM\r\n");
    return EFI_TIMEOUT;
}

/* ----------------------------------------------------------------
 * lme_send: send data through APF channel
 * ---------------------------------------------------------------- */
EFI_STATUS lme_send(LME_SESSION *lme, const UINT8 *data, UINT32 len)
{
    UINT32 total = 9 + len;
    UINT8 send_buf[4096];
    if (total > sizeof(send_buf)) {
        Print(L"LME: Send too large (%d)\r\n", total);
        return EFI_BUFFER_TOO_SMALL;
    }

    /* APF_CHANNEL_DATA: [94][recipient(4 BE)][length(4 BE)][data...] */
    send_buf[0] = APF_CHANNEL_DATA;
    send_buf[1] = (lme->amt_channel >> 24) & 0xFF;
    send_buf[2] = (lme->amt_channel >> 16) & 0xFF;
    send_buf[3] = (lme->amt_channel >> 8) & 0xFF;
    send_buf[4] = lme->amt_channel & 0xFF;
    send_buf[5] = (len >> 24) & 0xFF;
    send_buf[6] = (len >> 16) & 0xFF;
    send_buf[7] = (len >> 8) & 0xFF;
    send_buf[8] = len & 0xFF;
    for (UINT32 i = 0; i < len; i++)
        send_buf[9 + i] = data[i];

    Print(L"  [LME_SEND] %d bytes payload (APF total=%d)\r\n", len, total);

    return lme_raw_send(lme, send_buf, total);
}

/* ----------------------------------------------------------------
 * lme_receive: receive data from APF channel
 * ---------------------------------------------------------------- */
EFI_STATUS lme_receive(LME_SESSION *lme, UINT32 timeout_ms)
{
    UINT8 resp_buf[4096];
    UINT32 resp_len;

    lme->rx_len = 0;

    for (int attempts = 0; attempts < 50; attempts++) {
        EFI_STATUS status = lme_raw_recv(lme, resp_buf, sizeof(resp_buf), &resp_len);
        if (EFI_ERROR(status)) {
            if (lme->rx_len > 0)
                return EFI_SUCCESS;
            return status;
        }

        UINT8 msg_type = lme_process_apf(lme, resp_buf, resp_len);

        if (msg_type == APF_CHANNEL_DATA && lme->rx_len > 0) {
            /* Keep reading in case response spans multiple APF messages */
            continue;
        }

        if (msg_type == APF_CHANNEL_CLOSE) {
            if (lme->rx_len > 0)
                return EFI_SUCCESS;
            return EFI_ABORTED;
        }
    }

    if (lme->rx_len > 0)
        return EFI_SUCCESS;
    return EFI_TIMEOUT;
}

/* ----------------------------------------------------------------
 * lme_close
 * ---------------------------------------------------------------- */
EFI_STATUS lme_close(LME_SESSION *lme)
{
    Print(L"LME: Closing channel\r\n");

    if (lme->amt_channel != 0) {
        APF_CHANNEL_CLOSE_MSG close_msg;
        close_msg.message_type = APF_CHANNEL_CLOSE;
        close_msg.recipient_channel = be32(lme->amt_channel);
        lme_raw_send(lme, &close_msg, sizeof(close_msg));
    }

    heci_close(&lme->heci);
    return EFI_SUCCESS;
}
