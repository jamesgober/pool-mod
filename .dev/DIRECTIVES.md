# pool-mod - Directives

## Priority

1. `REPS.md` - SUPREME AUTHORITY
2. This file
3. `.dev/PROMPT.md`
4. `.dev/ROADMAP.md`

## Cross-platform

Linux, macOS, Windows. Platform-specific code feature-gated.

## REPS compliance

- Zero-allocation hot path
- Lock-free where contention matters
- `unsafe` only when measured and documented
- No `unwrap()` / `expect()` / `todo!()` / `unimplemented!()`
- No `print_stdout` / `print_stderr` / `dbg!()`
- All public items documented with one example

## Versioning

Fast-track to 1.0. Every release tagged. `.dev/release/v<version>.md` for each.

## Pre-1.0 audit

Mandatory. Checklist in `.dev/ROADMAP.md`. Report in `.dev/release/v1.0.0.md`.