#include "heci.h"

/* ----------------------------------------------------------------
 * Internal helpers
 * ---------------------------------------------------------------- */

/* Read a HECI MMIO register */
static inline UINT32 heci_reg_read(HECI_CONTEXT *ctx, UINT32 offset)
{
    return ctx->mmio[offset / sizeof(UINT32)];
}

/* Write a HECI MMIO register */
static inline void heci_reg_write(HECI_CONTEXT *ctx, UINT32 offset, UINT32 val)
{
    ctx->mmio[offset / sizeof(UINT32)] = val;
}

/* Stall for a number of microseconds */
static void heci_stall(HECI_CONTEXT *ctx, UINTN us)
{
    uefi_call_wrapper(ctx->ST->BootServices->Stall, 1, us);
}

/* Debug: dump H_CSR and ME_CSR_HA register values */
static void heci_dump_csr(HECI_CONTEXT *ctx, const CHAR16 *label)
{
    UINT32 h = heci_reg_read(ctx, H_CSR);
    UINT32 m = heci_reg_read(ctx, ME_CSR_HA);
    Print(L"  [CSR %s] H=0x%08x (RDY=%d RST=%d IG=%d IS=%d WP=%d RP=%d D=%d) "
          L"ME=0x%08x (RDY=%d WP=%d RP=%d D=%d)\r\n",
          label,
          h,
          (h >> 3) & 1, (h >> 4) & 1, (h >> 2) & 1, (h >> 1) & 1,
          CSR_GET_WP(h), CSR_GET_RP(h), CSR_GET_CBD(h),
          m,
          (m >> 3) & 1,
          CSR_GET_WP(m), CSR_GET_RP(m), CSR_GET_CBD(m));
}

/* Wait for ME to become ready (CSR_RDY set in ME_CSR_HA) */
static EFI_STATUS heci_wait_me_ready(HECI_CONTEXT *ctx)
{
    UINTN elapsed = 0;

    while (elapsed < HECI_TIMEOUT_US) {
        UINT32 me_csr = heci_reg_read(ctx, ME_CSR_HA);
        if (me_csr & CSR_RDY)
            return EFI_SUCCESS;
        heci_stall(ctx, HECI_POLL_US);
        elapsed += HECI_POLL_US;
    }

    Print(L"HECI: Timeout waiting for ME ready\r\n");
    return EFI_TIMEOUT;
}

/* Reset the host side of HECI and wait for ME ready */
static EFI_STATUS heci_reset(HECI_CONTEXT *ctx)
{
    UINT32 h_csr;

    Print(L"HECI: Resetting host interface...\r\n");

    /* Assert host reset + interrupt generate */
    h_csr = heci_reg_read(ctx, H_CSR);
    h_csr |= CSR_RST | CSR_IG;
    heci_reg_write(ctx, H_CSR, h_csr);

    heci_stall(ctx, 10000); /* 10ms for reset */

    /* Wait for ME to become ready */
    EFI_STATUS status = heci_wait_me_ready(ctx);
    if (EFI_ERROR(status))
        return status;

    /* Clear host reset, set ready + interrupt generate */
    h_csr = heci_reg_read(ctx, H_CSR);
    h_csr &= ~CSR_RST;
    h_csr |= CSR_RDY | CSR_IG;
    heci_reg_write(ctx, H_CSR, h_csr);

    /* Read buffer depth */
    h_csr = heci_reg_read(ctx, H_CSR);
    ctx->cb_depth = (UINT8)CSR_GET_CBD(h_csr);

    heci_dump_csr(ctx, L"after-reset");

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * Low-level send: write HECI header + payload into host CB
 * ---------------------------------------------------------------- */
EFI_STATUS heci_write_msg(HECI_CONTEXT *ctx, UINT8 me_addr,
                                 UINT8 host_addr, const void *data,
                                 UINT32 len, BOOLEAN complete)
{
    HECI_MSG_HDR hdr;
    UINT32 h_csr, me_csr;
    UINT32 empty_slots;
    UINT32 dwords = (len + 3) / 4;
    UINT32 total_dwords = 1 + dwords; /* header + payload */
    UINTN elapsed = 0;

    hdr.me_addr = me_addr;
    hdr.host_addr = host_addr;
    hdr.length = len;
    hdr.reserved = 0;
    hdr.msg_complete = complete ? 1 : 0;

    /* Wait for enough empty slots in host CB */
    while (elapsed < HECI_TIMEOUT_US) {
        h_csr = heci_reg_read(ctx, H_CSR);
        me_csr = heci_reg_read(ctx, ME_CSR_HA);

        if (!(me_csr & CSR_RDY)) {
            Print(L"  [SEND] ME not ready! ME_CSR=0x%08x\r\n", me_csr);
            return EFI_NOT_READY;
        }

        UINT32 wp = CSR_GET_WP(h_csr);
        UINT32 rp = CSR_GET_RP(h_csr);
        UINT32 depth = CSR_GET_CBD(h_csr);
        UINT32 filled = FILLED_SLOTS(wp, rp, depth);
        empty_slots = depth - filled - 1;

        if (empty_slots >= total_dwords)
            break;

        heci_stall(ctx, HECI_POLL_US);
        elapsed += HECI_POLL_US;
    }

    if (elapsed >= HECI_TIMEOUT_US)
        return EFI_TIMEOUT;

    /* Write header */
    heci_reg_write(ctx, H_CB_WW, *(UINT32 *)&hdr);

    /* Write payload dwords */
    const UINT8 *src = (const UINT8 *)data;
    for (UINT32 i = 0; i < dwords; i++) {
        UINT32 word = 0;
        UINT32 remaining = len - (i * 4);
        UINT32 copy_bytes = remaining < 4 ? remaining : 4;
        UINT8 *dst = (UINT8 *)&word;
        for (UINT32 j = 0; j < copy_bytes; j++)
            dst[j] = src[i * 4 + j];
        heci_reg_write(ctx, H_CB_WW, word);
    }

    /*
     * Set interrupt generate to notify ME.
     * Mask out H_IS (bit 1) which is write-1-to-clear — writing it
     * back would accidentally acknowledge a pending interrupt.
     */
    h_csr = heci_reg_read(ctx, H_CSR);
    h_csr |= CSR_IG;
    h_csr &= ~CSR_IS;
    heci_reg_write(ctx, H_CSR, h_csr);

    Print(L"  [SEND] OK, wrote %d dwords\r\n", total_dwords);

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * Low-level receive: read HECI header + payload from ME CB
 * ---------------------------------------------------------------- */
EFI_STATUS heci_read_msg(HECI_CONTEXT *ctx, void *buf,
                                UINT32 buf_size, UINT32 *out_len,
                                UINT8 *out_me_addr, UINT8 *out_host_addr)
{
    UINT32 me_csr;
    UINTN elapsed = 0;

    /* Wait for data in ME circular buffer */
    while (elapsed < HECI_TIMEOUT_US) {
        me_csr = heci_reg_read(ctx, ME_CSR_HA);
        UINT32 wp = CSR_GET_WP(me_csr);
        UINT32 rp = CSR_GET_RP(me_csr);
        if (wp != rp)
            break;

        /* Log progress every second */
        if (elapsed > 0 && (elapsed % 1000000) == 0)
            Print(L"  [RECV] Waiting... %ds (ME_CSR=0x%08x WP=%d RP=%d)\r\n",
                  elapsed / 1000000, me_csr, wp, rp);

        heci_stall(ctx, HECI_POLL_US);
        elapsed += HECI_POLL_US;
    }

    if (elapsed >= HECI_TIMEOUT_US) {
        Print(L"  [RECV] TIMEOUT after %ds\r\n", HECI_TIMEOUT_US / 1000000);
        heci_dump_csr(ctx, L"timeout");
        return EFI_TIMEOUT;
    }

    /* Read header from ME CB read window */
    UINT32 hdr_raw = heci_reg_read(ctx, ME_CB_RW);
    HECI_MSG_HDR *hdr = (HECI_MSG_HDR *)&hdr_raw;

    /* Verbose header logging removed — APF layer logs at higher level */

    if (out_me_addr)   *out_me_addr   = hdr->me_addr;
    if (out_host_addr) *out_host_addr = hdr->host_addr;

    UINT32 payload_len = hdr->length;
    if (out_len) *out_len = payload_len;

    if (payload_len > buf_size) {
        Print(L"  [RECV] Payload %d > buffer %d!\r\n", payload_len, buf_size);
        return EFI_BUFFER_TOO_SMALL;
    }

    /* Read payload dwords */
    UINT32 dwords = (payload_len + 3) / 4;
    UINT8 *dst = (UINT8 *)buf;

    for (UINT32 i = 0; i < dwords; i++) {
        UINT32 word = heci_reg_read(ctx, ME_CB_RW);
        UINT32 remaining = payload_len - (i * 4);
        UINT32 copy_bytes = remaining < 4 ? remaining : 4;
        UINT8 *s = (UINT8 *)&word;
        for (UINT32 j = 0; j < copy_bytes; j++)
            dst[i * 4 + j] = s[j];
    }

    /*
     * Acknowledge: set IG to notify ME we consumed data,
     * and clear IS (W1C) to acknowledge any pending interrupt.
     * This matches coreboot's heci_receive pattern.
     */
    UINT32 h_csr = heci_reg_read(ctx, H_CSR);
    h_csr |= CSR_IG | CSR_IS;   /* set IG + clear IS via W1C */
    heci_reg_write(ctx, H_CSR, h_csr);

    /* quiet — APF layer logs at higher level */

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * heci_init: find PCI device, map BAR0, reset
 * ---------------------------------------------------------------- */
EFI_STATUS heci_init(HECI_CONTEXT *ctx, EFI_SYSTEM_TABLE *SystemTable)
{
    EFI_STATUS status;
    UINT32 pci_addr;
    UINT32 bar0;
    UINT16 vendor_id;
    UINT16 cmd;

    ctx->ST = SystemTable;
    ctx->mmio = NULL;
    ctx->me_addr = 0;
    ctx->host_addr = 0;

    pci_addr = (1U << 31)
             | (HECI_BUS << 16)
             | (HECI_DEVICE << 11)
             | (HECI_FUNCTION << 8);

    /* Read Vendor ID */
    __asm__ volatile(
        "movl %1, %%eax\n\t"
        "movw $0xCF8, %%dx\n\t"
        "outl %%eax, %%dx\n\t"
        "movw $0xCFC, %%dx\n\t"
        "inl %%dx, %%eax\n\t"
        "movw %%ax, %0"
        : "=m"(vendor_id)
        : "r"(pci_addr | PCI_VENDOR_ID)
        : "eax", "edx"
    );

    if (vendor_id != INTEL_VENDOR_ID) {
        Print(L"HECI: Intel device not found at 00:16.0 (vendor=0x%04x)\r\n",
              vendor_id);
        return EFI_NOT_FOUND;
    }
    Print(L"HECI: Vendor ID=0x%04x (Intel)\r\n", vendor_id);

    /* Read BAR0 */
    __asm__ volatile(
        "movl %1, %%eax\n\t"
        "movw $0xCF8, %%dx\n\t"
        "outl %%eax, %%dx\n\t"
        "movw $0xCFC, %%dx\n\t"
        "inl %%dx, %%eax\n\t"
        "movl %%eax, %0"
        : "=m"(bar0)
        : "r"(pci_addr | PCI_BAR0)
        : "eax", "edx"
    );

    if (bar0 == 0 || bar0 == 0xFFFFFFFF) {
        Print(L"HECI: BAR0 not configured (0x%08x)\r\n", bar0);
        return EFI_NOT_FOUND;
    }

    /* Enable Memory Space access if not already set */
    __asm__ volatile(
        "movl %1, %%eax\n\t"
        "movw $0xCF8, %%dx\n\t"
        "outl %%eax, %%dx\n\t"
        "movw $0xCFC, %%dx\n\t"
        "inw %%dx, %%ax\n\t"
        "movw %%ax, %0"
        : "=m"(cmd)
        : "r"(pci_addr | PCI_COMMAND)
        : "eax", "edx"
    );

    Print(L"HECI: PCI CMD=0x%04x (MSE=%d)\r\n", cmd, (cmd & PCI_CMD_MSE) ? 1 : 0);

    if (!(cmd & PCI_CMD_MSE)) {
        cmd |= PCI_CMD_MSE;
        __asm__ volatile(
            "movl %0, %%eax\n\t"
            "movw $0xCF8, %%dx\n\t"
            "outl %%eax, %%dx\n\t"
            "movw $0xCFC, %%dx\n\t"
            "movw %1, %%ax\n\t"
            "outw %%ax, %%dx"
            :
            : "r"(pci_addr | PCI_COMMAND), "r"(cmd)
            : "eax", "edx"
        );
        Print(L"HECI: Enabled Memory Space access\r\n");
    }

    /* BAR0 is memory-mapped, mask lower bits to get base address */
    UINT64 mmio_base = (UINT64)(bar0 & 0xFFFFFFF0);

    /* Check if 64-bit BAR */
    if ((bar0 & 0x06) == 0x04) {
        UINT32 bar0_hi;
        __asm__ volatile(
            "movl %1, %%eax\n\t"
            "movw $0xCF8, %%dx\n\t"
            "outl %%eax, %%dx\n\t"
            "movw $0xCFC, %%dx\n\t"
            "inl %%dx, %%eax\n\t"
            "movl %%eax, %0"
            : "=m"(bar0_hi)
            : "r"(pci_addr | (PCI_BAR0 + 4))
            : "eax", "edx"
        );
        mmio_base |= ((UINT64)bar0_hi << 32);
        Print(L"HECI: 64-bit BAR, BAR0=0x%08x BAR1=0x%08x -> 0x%lx\r\n",
              bar0, bar0_hi, mmio_base);
    } else {
        Print(L"HECI: 32-bit BAR, base=0x%lx\r\n", mmio_base);
    }

    ctx->mmio = (volatile UINT32 *)(UINTN)mmio_base;

    Print(L"HECI: Found at PCI 00:16.0, BAR0=0x%lx\r\n", mmio_base);

    /* Reset HECI and wait for ME ready */
    status = heci_reset(ctx);
    if (EFI_ERROR(status)) {
        Print(L"HECI: Reset failed (status=0x%lx)\r\n", status);
        return status;
    }

    Print(L"HECI: ME ready, CB depth=%d\r\n", ctx->cb_depth);

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * heci_connect_amthi: HBM protocol to connect to AMTHI client
 * ---------------------------------------------------------------- */

/* Helper: compare two 16-byte UUIDs */
static BOOLEAN uuid_equal(const UINT8 *a, const UINT8 *b)
{
    for (int i = 0; i < 16; i++) {
        if (a[i] != b[i])
            return FALSE;
    }
    return TRUE;
}

/* Helper: print a UUID in standard format */
static void print_uuid(const UINT8 *u)
{
    Print(L"%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
          u[3], u[2], u[1], u[0],
          u[5], u[4],
          u[7], u[6],
          u[8], u[9],
          u[10], u[11], u[12], u[13], u[14], u[15]);
}

EFI_STATUS heci_connect_amthi(HECI_CONTEXT *ctx)
{
    EFI_STATUS status;
    UINT8 buf[256];
    UINT32 len;

    static const UINT8 amthi_uuid[16] = AMTHI_UUID_BYTES;

    /* --- Step 1: HBM Version Exchange --- */
    Print(L"HECI: Step 1 - HBM version exchange\r\n");
    {
        HBM_HOST_VERSION_REQ req = {0};
        req.cmd = HBM_CMD_HOST_VERSION;
        req.major = HBM_MAJOR_VERSION;
        req.minor = HBM_MINOR_VERSION;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) {
            Print(L"HECI: HBM version send failed\r\n");
            return status;
        }

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) {
            Print(L"HECI: HBM version recv failed\r\n");
            return status;
        }

        HBM_HOST_VERSION_RESP *resp = (HBM_HOST_VERSION_RESP *)buf;
        if (resp->cmd != HBM_CMD_HOST_VERSION_RESP || !resp->supported) {
            Print(L"HECI: HBM version not supported (cmd=0x%02x supp=%d)\r\n",
                  resp->cmd, resp->supported);
            return EFI_UNSUPPORTED;
        }
        Print(L"HECI: HBM version %d.%d OK\r\n", resp->major, resp->minor);
    }

    /* --- Step 2: Enumerate ME clients --- */
    Print(L"HECI: Step 2 - Enumerate clients\r\n");
    HBM_HOST_ENUM_RESP enum_resp;
    {
        HBM_HOST_ENUM_REQ req = {0};
        req.cmd = HBM_CMD_HOST_ENUM;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) return status;

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) return status;

        HBM_HOST_ENUM_RESP *resp = (HBM_HOST_ENUM_RESP *)buf;
        if (resp->cmd != HBM_CMD_HOST_ENUM_RESP) {
            Print(L"HECI: Enum failed (cmd=0x%02x)\r\n", resp->cmd);
            return EFI_DEVICE_ERROR;
        }

        for (int i = 0; i < 32; i++)
            enum_resp.valid_addresses[i] = resp->valid_addresses[i];

        UINT32 client_count = 0;
        for (UINT32 i = 1; i < 256; i++) {
            if (enum_resp.valid_addresses[i / 8] & (1 << (i % 8)))
                client_count++;
        }
        Print(L"HECI: Found %d ME clients\r\n", client_count);
    }

    /* --- Step 3: Query each client to find AMTHI --- */
    Print(L"HECI: Step 3 - Find AMTHI client\r\n");
    UINT8 found_me_addr = 0;
    BOOLEAN found = FALSE;

    Print(L"HECI: Looking for AMTHI UUID: ");
    print_uuid(amthi_uuid);
    Print(L"\r\n");

    for (UINT32 i = 1; i < 256 && !found; i++) {
        if (!(enum_resp.valid_addresses[i / 8] & (1 << (i % 8))))
            continue;

        HBM_CLIENT_PROP_REQ preq = {0};
        preq.cmd = HBM_CMD_HOST_CLIENT_PROP;
        preq.me_addr = (UINT8)i;

        status = heci_write_msg(ctx, 0, 0, &preq, sizeof(preq), TRUE);
        if (EFI_ERROR(status)) {
            Print(L"  ME[%d]: send failed (0x%lx)\r\n", i, status);
            continue;
        }

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) {
            Print(L"  ME[%d]: recv failed (0x%lx)\r\n", i, status);
            continue;
        }

        HBM_CLIENT_PROP_RESP *presp = (HBM_CLIENT_PROP_RESP *)buf;
        if (presp->cmd != HBM_CMD_HOST_CLIENT_PROP_RESP || presp->status != 0) {
            Print(L"  ME[%d]: bad response (cmd=0x%02x status=%d)\r\n",
                  i, presp->cmd, presp->status);
            continue;
        }

        Print(L"  ME[%d]: ", presp->me_addr);
        print_uuid(presp->uuid.b);
        Print(L" (max_msg=%d)\r\n", presp->max_msg_length);

        if (uuid_equal(presp->uuid.b, amthi_uuid)) {
            found_me_addr = presp->me_addr;
            ctx->max_msg_len = presp->max_msg_length;
            found = TRUE;
            Print(L"  ^^ AMTHI MATCH!\r\n");
        }
    }

    if (!found) {
        Print(L"HECI: AMTHI client not found among enumerated clients\r\n");
        return EFI_NOT_FOUND;
    }

    ctx->me_addr = found_me_addr;

    /* --- Step 4: Connect to AMTHI client --- */
    Print(L"HECI: Step 4 - Connect to AMTHI\r\n");
    {
        ctx->host_addr = 1;

        HBM_CONNECT_REQ req = {0};
        req.cmd = HBM_CMD_CONNECT;
        req.me_addr = ctx->me_addr;
        req.host_addr = ctx->host_addr;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) {
            Print(L"HECI: Connect send failed\r\n");
            return status;
        }

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) {
            Print(L"HECI: Connect recv failed\r\n");
            return status;
        }

        HBM_CONNECT_RESP *resp = (HBM_CONNECT_RESP *)buf;
        if (resp->cmd != HBM_CMD_CONNECT_RESP || resp->status != 0) {
            Print(L"HECI: Connect failed (cmd=0x%02x status=%d)\r\n",
                  resp->cmd, resp->status);
            return EFI_DEVICE_ERROR;
        }

        Print(L"HECI: Connected to AMTHI (ME:%d <-> Host:%d)\r\n",
              ctx->me_addr, ctx->host_addr);
    }

    /* --- Step 5: Consume initial flow control from ME --- */
    Print(L"HECI: Step 5 - Wait for initial flow control\r\n");
    {
        UINT8 fc_buf[64];
        UINT32 fc_len;
        UINT8 fc_me, fc_host;

        status = heci_read_msg(ctx, fc_buf, sizeof(fc_buf), &fc_len,
                               &fc_me, &fc_host);
        if (EFI_ERROR(status)) {
            Print(L"HECI: Warning - no initial flow control (0x%lx)\r\n", status);
            /* Non-fatal: continue anyway */
        } else {
            Print(L"HECI: Initial msg: me=%d host=%d cmd=0x%02x len=%d\r\n",
                  fc_me, fc_host, fc_buf[0], fc_len);
        }
    }

    heci_dump_csr(ctx, L"post-connect");

    return EFI_SUCCESS;
}

/* ----------------------------------------------------------------
 * Flow control helpers
 * ---------------------------------------------------------------- */

/* Send a flow control credit to ME on the HBM channel (me=0, host=0) */
static EFI_STATUS heci_send_flow_control(HECI_CONTEXT *ctx)
{
    HBM_FLOW_CONTROL fc = {0};
    fc.cmd = HBM_CMD_FLOW_CONTROL;
    fc.me_addr = ctx->me_addr;
    fc.host_addr = ctx->host_addr;

    Print(L"  [FC] Sending flow control credit for ME:%d Host:%d\r\n",
          ctx->me_addr, ctx->host_addr);

    return heci_write_msg(ctx, 0, 0, &fc, sizeof(fc), TRUE);
}

/* ----------------------------------------------------------------
 * heci_send / heci_receive: connected client communication
 *
 * The HECI/HBM protocol uses flow control credits:
 * - ME sends flow control on HBM channel after connect and after
 *   processing each command (granting host permission to send)
 * - Host must send flow control on HBM channel to grant ME
 *   permission to send a response
 *
 * In heci_receive, we must skip HBM messages (me=0/host=0)
 * that are flow control credits, and only return when we get
 * a message from our connected client.
 * ---------------------------------------------------------------- */
EFI_STATUS heci_send(HECI_CONTEXT *ctx, const void *data, UINT32 len)
{
    Print(L"  [heci_send] Sending %d bytes to ME:%d\r\n", len, ctx->me_addr);
    EFI_STATUS status = heci_write_msg(ctx, ctx->me_addr, ctx->host_addr,
                                       data, len, TRUE);
    if (EFI_ERROR(status))
        return status;

    /* Grant ME a credit to send the response back */
    return heci_send_flow_control(ctx);
}

EFI_STATUS heci_receive(HECI_CONTEXT *ctx, void *buf, UINT32 buf_size,
                        UINT32 *out_len)
{
    EFI_STATUS status;
    UINT8 msg_me_addr, msg_host_addr;
    UINT8 tmp_buf[256];
    UINT32 tmp_len;

    Print(L"  [heci_recv] Waiting for response (buf=%d)...\r\n", buf_size);
    heci_dump_csr(ctx, L"pre-recv");

    /*
     * Loop: read messages from ME CB, skipping any HBM messages
     * (flow control, notifications) until we get one addressed
     * to our connected client.
     */
    for (;;) {
        status = heci_read_msg(ctx, tmp_buf, sizeof(tmp_buf), &tmp_len,
                               &msg_me_addr, &msg_host_addr);
        if (EFI_ERROR(status))
            return status;

        /* Check if this is an HBM message (me=0, host=0) */
        if (msg_me_addr == 0 && msg_host_addr == 0) {
            UINT8 hbm_cmd = tmp_buf[0];
            Print(L"  [heci_recv] HBM message: cmd=0x%02x len=%d\r\n",
                  hbm_cmd, tmp_len);

            if (hbm_cmd == HBM_CMD_FLOW_CONTROL) {
                Print(L"  [heci_recv] Flow control from ME (for ME:%d Host:%d)\r\n",
                      tmp_buf[1], tmp_buf[2]);
            }
            /* HBM_CLIENT_DISCONNECT_REQ (0x07) — ME is requesting disconnect.
               Log details and respond with HBM_CLIENT_DISCONNECT_RES (0x87). */
            else if (hbm_cmd == 0x07 && tmp_len >= 4) {
                Print(L"  [heci_recv] HBM DISCONNECT REQ: me_addr=%d host_addr=%d status=%d\r\n",
                      tmp_buf[1], tmp_buf[2], tmp_buf[3]);
                /* Respond with disconnect response */
                UINT8 disc_resp[4];
                disc_resp[0] = 0x87;         /* HBM_CLIENT_DISCONNECT_RES */
                disc_resp[1] = tmp_buf[1];   /* me_addr */
                disc_resp[2] = tmp_buf[2];   /* host_addr */
                disc_resp[3] = 0;            /* status = success */
                heci_write_msg(ctx, 0, 0, disc_resp, 4, TRUE);
                Print(L"  [heci_recv] Sent disconnect response\r\n");
            }
            continue; /* keep waiting for actual client message */
        }

        /* Check if addressed to us */
        if (msg_me_addr == ctx->me_addr && msg_host_addr == ctx->host_addr) {
            Print(L"  [heci_recv] Got client message: %d bytes\r\n", tmp_len);

            if (tmp_len > buf_size)
                return EFI_BUFFER_TOO_SMALL;

            /* Copy to caller's buffer */
            UINT8 *dst = (UINT8 *)buf;
            for (UINT32 i = 0; i < tmp_len; i++)
                dst[i] = tmp_buf[i];

            if (out_len)
                *out_len = tmp_len;

            return EFI_SUCCESS;
        }

        /* Unexpected source - log and skip */
        Print(L"  [heci_recv] Unexpected msg from ME:%d Host:%d (len=%d), skipping\r\n",
              msg_me_addr, msg_host_addr, tmp_len);
    }
}

/* ----------------------------------------------------------------
 * heci_close
 * ---------------------------------------------------------------- */
void heci_close(HECI_CONTEXT *ctx)
{
    Print(L"HECI: Closing\r\n");
    ctx->mmio = NULL;
    ctx->me_addr = 0;
    ctx->host_addr = 0;
}

/* ----------------------------------------------------------------
 * heci_connect_client: generic version of heci_connect_amthi
 *
 * Connects to any ME client by UUID. Performs the full HBM flow:
 * version exchange, enumerate, find client, connect.
 * Expects a freshly initialized (heci_init'd) context.
 * ---------------------------------------------------------------- */
EFI_STATUS heci_connect_client(HECI_CONTEXT *ctx, const UINT8 target_uuid[16])
{
    EFI_STATUS status;
    UINT8 buf[256];
    UINT32 len;

    Print(L"HECI: Connecting to client UUID: ");
    print_uuid(target_uuid);
    Print(L"\r\n");

    /* --- HBM Version Exchange --- */
    {
        HBM_HOST_VERSION_REQ req = {0};
        req.cmd = HBM_CMD_HOST_VERSION;
        req.major = HBM_MAJOR_VERSION;
        req.minor = HBM_MINOR_VERSION;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) return status;

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) return status;

        HBM_HOST_VERSION_RESP *resp = (HBM_HOST_VERSION_RESP *)buf;
        if (resp->cmd != HBM_CMD_HOST_VERSION_RESP || !resp->supported)
            return EFI_UNSUPPORTED;
        Print(L"HECI: HBM version %d.%d OK\r\n", resp->major, resp->minor);
    }

    /* --- Enumerate ME clients --- */
    HBM_HOST_ENUM_RESP enum_resp;
    {
        HBM_HOST_ENUM_REQ req = {0};
        req.cmd = HBM_CMD_HOST_ENUM;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) return status;

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) return status;

        HBM_HOST_ENUM_RESP *resp = (HBM_HOST_ENUM_RESP *)buf;
        if (resp->cmd != HBM_CMD_HOST_ENUM_RESP)
            return EFI_DEVICE_ERROR;

        for (int i = 0; i < 32; i++)
            enum_resp.valid_addresses[i] = resp->valid_addresses[i];
    }

    /* --- Find target client --- */
    UINT8 found_me_addr = 0;
    BOOLEAN found = FALSE;

    for (UINT32 i = 1; i < 256 && !found; i++) {
        if (!(enum_resp.valid_addresses[i / 8] & (1 << (i % 8))))
            continue;

        HBM_CLIENT_PROP_REQ preq = {0};
        preq.cmd = HBM_CMD_HOST_CLIENT_PROP;
        preq.me_addr = (UINT8)i;

        status = heci_write_msg(ctx, 0, 0, &preq, sizeof(preq), TRUE);
        if (EFI_ERROR(status)) continue;

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) continue;

        HBM_CLIENT_PROP_RESP *presp = (HBM_CLIENT_PROP_RESP *)buf;
        if (presp->cmd != HBM_CMD_HOST_CLIENT_PROP_RESP || presp->status != 0)
            continue;

        if (uuid_equal(presp->uuid.b, target_uuid)) {
            found_me_addr = presp->me_addr;
            ctx->max_msg_len = presp->max_msg_length;
            found = TRUE;
            Print(L"HECI: Found target client at ME addr %d (max_msg=%d)\r\n",
                  found_me_addr, ctx->max_msg_len);
        }
    }

    if (!found) {
        Print(L"HECI: Target client not found\r\n");
        return EFI_NOT_FOUND;
    }

    ctx->me_addr = found_me_addr;

    /* --- Connect --- */
    {
        /* Use host_addr=2 to avoid collision with prior AMTHI connection
           (host_addr=1) that the ME might not have fully cleaned up */
        ctx->host_addr = 2;

        HBM_CONNECT_REQ req = {0};
        req.cmd = HBM_CMD_CONNECT;
        req.me_addr = ctx->me_addr;
        req.host_addr = ctx->host_addr;

        status = heci_write_msg(ctx, 0, 0, &req, sizeof(req), TRUE);
        if (EFI_ERROR(status)) return status;

        status = heci_read_msg(ctx, buf, sizeof(buf), &len, NULL, NULL);
        if (EFI_ERROR(status)) return status;

        HBM_CONNECT_RESP *resp = (HBM_CONNECT_RESP *)buf;
        if (resp->cmd != HBM_CMD_CONNECT_RESP || resp->status != 0) {
            Print(L"HECI: Connect failed (status=%d)\r\n", resp->status);
            return EFI_DEVICE_ERROR;
        }

        Print(L"HECI: Connected (ME:%d <-> Host:%d)\r\n",
              ctx->me_addr, ctx->host_addr);
    }

    /* Consume initial flow control */
    {
        UINT8 fc_buf[64];
        UINT32 fc_len;
        status = heci_read_msg(ctx, fc_buf, sizeof(fc_buf), &fc_len, NULL, NULL);
        if (!EFI_ERROR(status))
            Print(L"HECI: Initial flow control consumed (cmd=0x%02x)\r\n", fc_buf[0]);
    }

    return EFI_SUCCESS;
}
