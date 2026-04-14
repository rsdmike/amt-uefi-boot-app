---
name: AMT CCM provisioning architecture from RPC-Go
description: How CCM activation actually works based on the working RPC-Go implementation - HECI→LME→APF→WSMAN flow
type: project
---

## CCM Activation Flow (from RPC-Go)

The CCM provisioning is NOT just raw PTHI commands. It uses a layered protocol stack:

```
HECI (hardware) → PTHI (get LSA creds, check state)
HECI (hardware) → LME (APF protocol) → HTTP/WSMAN → HostBasedSetupService
```

### Exact sequence:
1. Open HECI, connect to PTHI client (GUID 12F80028-B4B7-4B2D-ACA8-46E0FF65814C)
2. Check if AMT is enabled (IsChangeToAMTEnabled via watchdog interface)
3. If disabled, enable via SetAmtOperationalState
4. GetLocalSystemAccount → gets LSA username/password for initial WSMAN auth
5. Close PTHI, reconnect HECI to LME client (GUID 6733A4DB-0476-4E7B-B3AF-BCFC29BEE7A7)
6. Establish APF channel over LME (APF_PROTOCOL_VERSION → APF_CHANNEL_OPEN)
7. Send WSMAN/HTTP requests through APF tunnel:
   a. GetGeneralSettings → gets DigestRealm
   b. HostBasedSetupService(DigestRealm, NewAMTPassword) → activates CCM
8. Verify: GetControlMode should return 1 (CCM)
9. Optional: CommitChanges if TLS enforced

### Key insight for UEFI:
In UEFI we don't have /dev/mei0 or Windows MEI driver. We must talk to HECI directly via PCI MMIO (Bus 0, Dev 22, Fn 0, BAR0). The PTHI part is straightforward binary protocol. The LME/APF/WSMAN part is significantly more complex — requires HTTP client, XML parsing, Digest auth.

### Alternative UEFI approach:
Could potentially use StartConfigurationHBased PTHI command (0x0400008B) to bypass WSMAN entirely, but RPC-Go uses the WSMAN path. Need to investigate if PTHI-only CCM activation is possible.

**Why:** Understanding the real protocol stack is essential before writing code.
**How to apply:** Phase 1 should validate HECI+PTHI works. Then decide whether to implement full WSMAN stack or find PTHI-only CCM path.
