# pool-mod - Roadmap to 1.0

Fast-track. No slow-stepping.

---

## Phase 0.1.0 - Scaffold (done)

- [x] Repository created
- [x] Cargo.toml, README, LICENSE x2, CHANGELOG
- [x] REPS.md
- [x] CI workflow (Linux/macOS/Windows on stable + MSRV, Node 24)
- [x] Initial commit pushed

---

## Phase 0.2.0 - Foundation

Define the public API surface.

Skill areas in scope:

  - pooling algorithms
  - resource lifecycle
  - validation-on-borrow
  - health monitoring

- [ ] Public types defined
- [ ] Public traits defined
- [ ] Module structure laid out
- [ ] Error type defined
- [ ] First end-to-end smoke test passing
- [ ] CHANGELOG updated
- [ ] `.dev/release/v0.2.0.md` written

---

## Phase 0.5.0 - Implementation

- [ ] All public API methods implemented (no `todo!()`)
- [ ] Property tests for state machines / invariants
- [ ] Integration tests
- [ ] Basic benchmarks
- [ ] Documentation drafted
- [ ] No `unwrap` / `expect` outside of tests
- [ ] CHANGELOG updated
- [ ] `.dev/release/v0.5.0.md` written

---

## Phase 0.9.0 - Hardening + Audit

Feature freeze. Quality focus.

### Audit checklist (mandatory)

#### Feature completeness
- [ ] Every roadmap item delivered
- [ ] Every README claim verified

#### Code cleanliness
- [ ] No dead code
- [ ] No commented-out code
- [ ] No TODO/FIXME without tracking issue
- [ ] No `#[allow(...)]` without justification

#### Error hardening
- [ ] Every public function: all error paths documented
- [ ] Every error variant: documented + tested
- [ ] No panics in shipping code paths
- [ ] Error messages actionable

#### API stability
- [ ] Every public item reviewed for 1.0
- [ ] Sealed traits where appropriate
- [ ] `#[non_exhaustive]` on growth-likely enums

#### Documentation
- [ ] Every public item: rustdoc with example
- [ ] README accurate
- [ ] CHANGELOG complete
- [ ] `cargo doc` zero warnings

#### Tests
- [ ] Unit test coverage on all public functions
- [ ] Integration tests
- [ ] Property tests for invariants
- [ ] Cross-platform CI green
- [ ] Both stable and MSRV green

#### Performance
- [ ] Hot paths benchmarked
- [ ] Allocation profile checked
- [ ] No regressions
- [ ] Benchmark baselines saved

#### Final
- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test --all-features` clean
- [ ] `cargo doc` clean with `RUSTDOCFLAGS=-D warnings`

### Output
- [ ] `.dev/release/v0.9.0.md` written
- [ ] Audit findings logged
- [ ] All findings resolved or deferred to 1.x

---

## Phase 0.9.x - Audit fixes

- [ ] All 0.9.0 blockers resolved
- [ ] No new features
- [ ] Final benchmarks recorded
- [ ] Final API freeze

---

## Phase 1.0.0 - Stable release

- [ ] All 0.9.x findings resolved
- [ ] Final API freeze
- [ ] Final benchmarks captured
- [ ] `.dev/release/v1.0.0.md` written
- [ ] Tag `v1.0.0` on main
- [ ] Publish to crates.io