---
name: Cannot run WSL commands from Claude
description: Build commands (make, gcc) run in WSL which Claude cannot access directly - user must run them
type: feedback
---

Claude Code runs in the Windows environment and cannot execute WSL commands. Build commands like `make`, `gcc`, etc. must be run by the user in their WSL terminal.

**Why:** Claude's Bash tool runs in Windows context, not inside WSL.
**How to apply:** When build/test steps are needed, provide the commands for the user to run rather than attempting to execute them. Focus on writing files and providing instructions.
