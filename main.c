#include <efi.h>
#include <efilib.h>
#include "amt.h"
#include "lme.h"

EFI_STATUS
EFIAPI
efi_main(EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE *SystemTable)
{
    EFI_INPUT_KEY Key;
    UINTN Index;
    HECI_CONTEXT heci;
    EFI_STATUS status;
    AMT_LSA_CREDENTIALS lsa;
    LME_SESSION lme;

    InitializeLib(ImageHandle, SystemTable);

    uefi_call_wrapper(SystemTable->ConOut->ClearScreen, 1, SystemTable->ConOut);

    Print(L"Hello, UEFI World!\r\n");
    Print(L"========================================\r\n");
    Print(L"AMT CCM Provisioning - Phase 2\r\n");
    Print(L"========================================\r\n\r\n");

    /* ============================================================
     * PHASE 1: PTHI — query AMT status and get LSA credentials
     * ============================================================ */

    Print(L"[1/6] Initializing HECI...\r\n");
    status = heci_init(&heci, SystemTable);
    if (EFI_ERROR(status)) {
        Print(L"FAILED: heci_init (0x%lx)\r\n", status);
        goto wait_exit;
    }

    Print(L"[2/6] Connecting to AMTHI...\r\n");
    status = heci_connect_amthi(&heci);
    if (EFI_ERROR(status)) {
        Print(L"FAILED: heci_connect_amthi (0x%lx)\r\n", status);
        goto cleanup_heci;
    }

    Print(L"[3/6] Querying AMT status...\r\n");
    {
        UINT32 control_mode;
        status = amt_get_control_mode(&heci, &control_mode);
        if (!EFI_ERROR(status)) {
            Print(L"  Control Mode: %s (%d)\r\n",
                  amt_control_mode_str(control_mode), control_mode);
        }

        UINT32 prov_state;
        status = amt_get_provisioning_state(&heci, &prov_state);
        if (!EFI_ERROR(status))
            Print(L"  Prov. State:  %d\r\n", prov_state);

        UINT8 uuid[16];
        status = amt_get_uuid(&heci, uuid);
        if (!EFI_ERROR(status)) {
            Print(L"  UUID:         %02x%02x%02x%02x-%02x%02x-%02x%02x-"
                  L"%02x%02x-%02x%02x%02x%02x%02x%02x\r\n",
                  uuid[3], uuid[2], uuid[1], uuid[0],
                  uuid[5], uuid[4], uuid[7], uuid[6],
                  uuid[8], uuid[9],
                  uuid[10], uuid[11], uuid[12], uuid[13],
                  uuid[14], uuid[15]);
        }
    }

    Print(L"[4/6] Getting Local System Account...\r\n");
    status = amt_get_local_system_account(&heci, &lsa);
    if (EFI_ERROR(status)) {
        Print(L"FAILED: amt_get_local_system_account (0x%lx)\r\n", status);
        goto cleanup_heci;
    }
    {
        int pwd_len = 0;
        while (pwd_len < LSA_PASSWORD_LEN && lsa.password[pwd_len]) pwd_len++;
        Print(L"  LSA User: %a  Pass: (%d chars)\r\n", lsa.username, pwd_len);
    }

    /* ============================================================
     * PHASE 2: LME/APF — open tunnel to AMT HTTP server
     * ============================================================ */

    Print(L"\r\n[5/6] Initializing LME (APF tunnel)...\r\n");
    status = lme_init(&lme, &heci, SystemTable);
    if (EFI_ERROR(status)) {
        Print(L"FAILED: lme_init (0x%lx)\r\n", status);
        goto cleanup_heci;
    }

    Print(L"[6/6] Opening APF channel to port %d...\r\n", APF_AMT_HTTP_PORT);
    status = lme_channel_open(&lme);
    if (EFI_ERROR(status)) {
        Print(L"FAILED: lme_channel_open (0x%lx)\r\n", status);
        goto cleanup_lme;
    }

    Print(L"\r\n========================================\r\n");
    Print(L"Phase 2 complete!\r\n");
    Print(L"  AMTHI PTHI:  OK\r\n");
    Print(L"  LSA creds:   OK\r\n");
    Print(L"  LME/APF:     OK (channel=%d)\r\n", lme.amt_channel);
    Print(L"  Next: WSMAN over APF for CCM activation\r\n");
    Print(L"========================================\r\n");
    Print(L"Press any key to exit.\r\n");

cleanup_lme:
    lme_close(&lme);
    goto wait_exit;

cleanup_heci:
    heci_close(&heci);

wait_exit:
    uefi_call_wrapper(SystemTable->ConIn->Reset, 2, SystemTable->ConIn, FALSE);
    uefi_call_wrapper(SystemTable->BootServices->WaitForEvent, 3,
                      1, &SystemTable->ConIn->WaitForKey, &Index);
    uefi_call_wrapper(SystemTable->ConIn->ReadKeyStroke, 2,
                      SystemTable->ConIn, &Key);

    return EFI_SUCCESS;
}
