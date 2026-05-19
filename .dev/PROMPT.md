# pool-mod - Project Prompt

## Priority order

1. `REPS.md` - SUPREME AUTHORITY
2. `.dev/DIRECTIVES.md`
3. This file (`.dev/PROMPT.md`)
4. `.dev/ROADMAP.md`

## What this crate is

Generic object and connection pooling. Async-safe with min/max sizing, idle timeouts, max-lifetime enforcement, validation-on-borrow, and health-check callbacks. Works for database connections, HTTP clients, worker threads, or any expensive resource.

## Why it exists

Async-safe min/max sizing, idle timeouts, max-lifetime enforcement, validation-on-borrow, health-check callbacks. Runtime-agnostic.

## Skill areas

- pooling algorithms
- resource lifecycle
- validation-on-borrow
- health monitoring

## Scope (1.0)

Defined in `.dev/ROADMAP.md`.

## Out of scope (always)

- Features requiring async runtime hard-dependency
- Features pulling in heavy framework dependency
- Features that violate REPS

## Pre-1.0 audit (mandatory)

See `.dev/ROADMAP.md` for the audit checklist. Must verify:

- Feature completeness vs. the roadmap
- API accuracy and stability
- Code cleanliness (no dead code, no commented-out blocks, no TODOs)
- Error hardening
- Documentation completeness
- Test coverage
- Benchmark coverage
- Cross-platform CI passing

## Versioning

Fast-track. No slow-stepping:

- 0.1.0 - scaffold
- 0.2.0 - first real implementation
- 0.5.0 - most features in place
- 0.9.0 - feature-complete, hardening
- 0.9.x - audit findings
- 1.0.0 - stable