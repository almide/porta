<!-- description: Align native sandbox messaging with actual enforcement level -->

# Sandbox Honesty

**Priority: Medium**

## Problem

Native sandbox uses `(allow default)` which is broadly permissive. The product says "sandbox runtime" but the actual enforcement is closer to "constrained runtime."

## Fix

Either strengthen the sandbox (move toward deny-default for FS read) or align the messaging. At minimum, documentation and CLI help should accurately describe what the sandbox does and doesn't enforce.

## Files
- `src/cli.almd` — update help text
- README.md / docs — accurate capability description
