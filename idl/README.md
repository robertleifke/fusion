# Fusion IDL

This directory stores generated IDLs and lightweight client artifacts.

## Suggested workflow

1. Build or test the program.
2. Generate/update `fusion.json` IDL.
3. Regenerate SDK bindings from that IDL.

Keep IDL generation deterministic in CI so `sdk/` matches onchain instruction
and account layouts.
