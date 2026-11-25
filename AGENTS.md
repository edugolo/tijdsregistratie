# AGENTS.md

Instructions for coding agents (and humans) working in this repository.

## What this is

Open-source, self-hosted time registration for schools and school groups with multiple sites.
Employees start and stop their working day with one tap in an installable web app (PWA), with a
location check at the moment of clocking. Managers enroll employees in person and follow up their
own sites. One deployment per organisation.

Read before working:

- `docs/plan.md` — scope, architecture, data model, task list. Source of truth for decisions.
- `DESIGN.md` — UX principles, visual language, copy rules, screen inventory.
- `docs/design/` — mockups of the flows and screens.

## Stack

- Rust workspace, Dioxus fullstack (web target: WASM client + axum server via server functions).
- PostgreSQL with `sqlx` (compile-time checked queries, migrations in `migrations/`).
- Passkeys with `webauthn-rs`; sessions with `tower-sessions` (PostgreSQL store).
- Field encryption with XChaCha20-Poly1305 (`chacha20poly1305`).

## Layout

```
crates/domain/   pure logic (zone check, day totals, break deduction); no IO, no async
crates/crypto/   field encryption, Encrypted<T>, key loading and rotation
crates/app/      Dioxus fullstack app; src/client (UI), src/server (server functions, auth, repository)
crates/admin/    operator CLI (bootstrap admin, seed sites, purge coordinates, rotate key)
migrations/      sqlx migrations
config/example/  example configuration
public/          manifest, icons, service worker
tests/e2e/       browser tests
deploy/          container image, compose file, operator docs
```

## Commands

Configuration comes from environment variables; see `config/example/`. Never hard-code
machine-specific paths, hosts or ports.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p domain -p crypto            # fast, no database
DATABASE_URL=postgres://... cargo test -p app   # needs a throwaway PostgreSQL database
cargo sqlx migrate run                    # apply migrations
cargo sqlx prepare --workspace            # refresh offline query metadata after changing queries
dx serve --package app                    # dev server
```

Run the tests for what you touched; the full suite runs in CI.

## Rules

### Language

- Code, identifiers, comments, commit messages and docs: **English**.
- User-facing strings: **Flemish (nl-BE)** first, via the i18n module only; never inline UI text in
  components. Register: informal "je". See `DESIGN.md` for copy rules.

### Security and privacy (non-negotiable)

- **No plaintext personal data in the database.** Names, e-mail, coordinates and any free text are
  stored as `Encrypted<T>` with associated data `table || column || row id`. Only ids, timestamps,
  enums and site configuration are plaintext. If you add a personal field, encrypt it.
- Never log decrypted personal data, tokens, session ids or credentials.
- The encryption key never goes into the database, the repository, test fixtures or logs.
- Every server function authenticates and checks role and site scope **on the server**. The client
  is never trusted for identity, role, time or zone status.
- `time_event` is **append-only**. Corrections are new rows referencing the original, with a reason.
- Every manager/admin mutation writes an `audit_log` row (ids only, no plaintext personal data).
- Enrollment tokens: random 256-bit, stored hashed, single use, short-lived, bound to one person.
  The typed 12-symbol code is a second, rate-limited 60-bit credential on the same row (same
  person, expiry and single use), stored only as an HMAC keyed with the blind-index key.
- The server timestamp is authoritative; the client timestamp is informational.
- No secrets, real names, real addresses or real coordinates in code, tests or fixtures. Use
  obviously fake data (`example.org`, fictional names, fictional sites).

### Code

- `crates/domain` stays pure: no IO, no database, no clock (pass time in).
- Prefer small, typed functions over clever abstractions. No dead code, no commented-out code.
- No `unwrap()`/`expect()` in server code paths that handle requests; return typed errors.
- Fix lints at the root cause; do not silence them with `allow` attributes unless justified in a comment.
- Keep organisation-specific values (names, colours, sites, durations) in configuration.

### Tests

- Test-driven: write the failing test first, then the code.
- `domain` and `crypto`: unit tests including edge cases (zone boundary, poor accuracy, wrong
  associated data, key rotation).
- Server: integration tests with `sqlx::test`, including a check that no table contains plaintext
  personal data after a full flow.
- Browser tests use a virtual WebAuthn authenticator and geolocation overrides.

### Git

- Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`).
- One logical change per commit; stage explicit paths.
- Never commit `.env`, keys, database dumps or confidential source documents.

## Definition of done

- Tests for the change exist and pass; `fmt` and `clippy` are clean.
- No new plaintext personal data, scope checks present on new server functions.
- UI strings go through i18n and follow `DESIGN.md`.
- `docs/plan.md` updated if a decision or the data model changed.
