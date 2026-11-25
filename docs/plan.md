# Tijdsregistratie — plan (v0.4)

Open-source time registration for schools and school groups with multiple sites. Built for the Belgian
obligation of objective, reliable and accessible working-time registration (expected from
1 January 2027, transition until 1 April 2027). Designs: `docs/design/`.

## 1. Principles

- **Super simple for the employee.** Start or stop in at most 2–3 taps, under 10 seconds. Big
  buttons, very simple language, no technical words. Many users have low digital skills or another
  home language.
- **Self-hosted, one deployment per organisation.** Data stays with the school; each deployment has
  its own domain. Shipped as a container image plus a compose file (app + PostgreSQL).
- **Everything organisation-specific is configuration**: name, logo, colours, sites, break rules,
  retention periods, zone policy. No organisation names or ids in code.
- **No external identity provider required.** Everyone (employees, managers, admins) logs in with
  a passkey. Roles live in the app.
- **A stolen database is useless.** Personal data is encrypted at the application level with a key
  that never lives in the database (§6).
- **Modular.** Core (time registration, location, roles) stays stable; later modules (corrections,
  validation, payroll export, absence, training) are added without rebuilding the core.
- **Licence:** MIT (`LICENSE`). Repository: GitHub `edugolo/tijdsregistratie`.

## 2. Scope

### Version 1

Employee:
1. One-time enrollment in person with their manager (QR code), install the PWA, create a passkey.
2. Log in with the passkey (face, fingerprint or device PIN).
3. Accept location once.
4. Start / stop with one big button. Fixed breaks are deducted automatically, not clocked.

Manager (scoped to their own site or sites):
1. "Mijn team": add / deactivate employees, link a device (QR), revoke a device.
2. Today's status per employee: started, stopped, not started, out of zone.

Admin (whole organisation):
1. Sites: name, address, coordinates, radius, break rule.
2. People and roles: make someone manager of one or more sites, or admin.
3. Same team screens as a manager, for every site.

First-run setup (onboarding) in the web app, see §2.1. The admin CLI `bootstrap-admin` stays as a
fallback (lost admin passkeys, headless installs).

### 2.1 First-run setup

When the database has no active admin, the app is in **setup mode**: every route redirects to
`/setup`. Nobody else can use the app until setup is finished.

- **Protection:** at startup in setup mode the server generates a one-time setup code (random,
  stored hashed, valid until setup completes or the server restarts) and writes it to the server log
  (`Setup code: …`); `admin setup-code` prints a fresh one. The first step asks for this code, so a
  stranger who reaches the URL first cannot claim the installation.
- **Steps** (one question per screen, can go back):
  1. S1 Welkom + setup code.
  2. S2 Organisatie: name (e.g. scholengroep or school), short name for the app header, brand colour
     (preset swatches + custom), optional logo upload (PNG/SVG, max 512 KB), time zone
     (default Europe/Brussels).
  3. S3 Eerste school (site): name, address, location by "Gebruik mijn huidige locatie" (stand at the
     school) or entering coordinates, radius (default 150 m), fixed break (default 30 min after 6 h).
     More sites later in A1.
  4. S4 Jij: your name. You become admin (and optionally manager of the first site).
  5. S5 Maak je sleutel aan: passkey registration on this device; the setup code is consumed in the
     same transaction that creates the admin, so setup can only complete once.
  6. S6 Klaar: next steps (add managers, add employees, link devices) with links to A2 / M1.
- Afterwards, admins edit the same organisation fields in **A0 Organisatie** (settings screen).
- Address search is optional: the server queries a Nominatim-compatible geocoder (`GEOCODER_URL`, default the public OpenStreetMap instance, `off` disables it), admin-only (and during setup after a valid setup code), globally limited to 1 request/s with a 24 h cache. A school address is not personal data; no API keys. See `docs/operations.md`.

### Next (needed for legal compliance, after version 1)

Corrections with a mandatory reason and audit trail, validation by the manager before export,
payroll export (registered time or contract time; first exporter: Acerta), contract hours per
employee, contract types, internal messages visible on login, signals for missing registrations.

### Later

Absence, leave and attests; replacement signals; training counters; kiosk mode (shared tablet per
site); optional OIDC login for managers (identity only, roles stay in the app).

## 3. Stack

| Concern | Choice |
|---|---|
| App | Rust, Dioxus fullstack (web target: WASM client + axum server, server functions), `dioxus` 0.7.x stable |
| DB | PostgreSQL + `sqlx`, migrations in `migrations/`, offline query metadata committed |
| Passkeys | `webauthn-rs` 0.6.x on the server; `navigator.credentials` via `web-sys` / small JS glue |
| Sessions | `tower-sessions` with a PostgreSQL store; HTTP-only, Secure, SameSite=Lax cookies |
| Encryption | RustCrypto `chacha20poly1305` (XChaCha20-Poly1305) + `hmac`/`sha2` for blind indexes |
| Location | browser Geolocation API via `web-sys`; zone check on the server |
| PWA | hand-written `manifest.webmanifest` + service worker in `public/` |
| Theme | Material 3 `SchemeFidelity` from the brand colour (`crates/theme`, crate `material-colors`), served as `/theme.css`; light/dark/system per device (DESIGN.md section 4) |

Exact versions are pinned in `Cargo.toml`.

i18n: i18n-embed + `fl!`. Messages live in `crates/app/i18n/<locale>/app.ftl`; add a language by adding `i18n/<locale>/app.ftl`. nl-BE is the fallback.

## 4. Identity and roles

### 4.1 One login method for everyone: passkeys

- A person is enrolled **once, in person**, by someone who knows them: a manager for employees, an
  admin for managers, the CLI for the first admin.
- The enroller taps "Koppel toestel" → the server creates an enrollment token: 256-bit random,
  stored hashed, **15 minutes valid, single use, bound to one person**, `issued_by` recorded.
- The enroller's screen shows a QR code with `https://<domain>/koppel#<token>` (fragment: never sent to servers or proxies). The person scans it
  with their own phone, installs the PWA and registers a passkey (user verification required,
  resident key preferred so login is discoverable).
- The enroller's screen polls and switches to "Gekoppeld".
- Login: discoverable passkey authentication, one tap.
- Sessions: employees 90 days sliding; managers and admins one working day (max 12 h).
- New or lost phone: same flow; managers and admins can revoke passkeys.
- Devices: employees have exactly one device (a new enrollment revokes the previous passkey in the
  same transaction). Managers, HR and admins may link several (phone and desktop): enrolling adds a
  passkey, each can be revoked on its own (only its sessions end; the last working admin passkey
  stays), and staff link another device for themselves under Instellingen, "Mijn toestellen".
- WebAuthn user handle is a random id, never the name. The display name is sent to the
  authenticator (it lives on the person's own device) but never stored in plaintext on the server.

### 4.2 Roles

- `role_assignment(person_id, role, site_id)`: `employee` at a site, `manager` of a site,
  `hr` (personeelsdienst) and `admin` of the organisation (site null).
- A person can have several roles (a manager who also clocks their own time).
- Employee: own events only. Manager: people and events of their own sites only. Admin: everything.
- HR (personeelsdienst), decided 2026-11-24: sees Vandaag and Mensen for all sites, and (when built)
  views, corrects and exports hours for all sites; adds people and links devices for employees and
  managers at any site; cannot grant the admin or HR role, cannot change settings, sites or branding.
  Only admins grant admin/HR.
  Implementation: HR may set a person's role between employee and manager and their sites, link and
  revoke devices and deactivate, but not for people who hold the admin or HR role (those are
  view-only for HR). Settings, sites, branding and geocoding stay admin only; HR only gets
  "Weergave" under Instellingen. HR does not count as a working admin for the lockout protection.
  The scope is `StaffScope { kind: Admin | Hr | Manager, site_ids }` with `can_view_all_sites()`,
  `can_manage_settings()` and `can_grant_role(role)`; future hour views and exports for all sites
  check `can_view_all_sites()`.
- Every server function checks scope on the server.
- Every manager/admin mutation writes an audit row (who, what, when; ids only, no plaintext personal data).
  Role and site changes record the new role (`role:<name>`) and the added/removed site ids
  (`added:<id>`, `removed:<id>`) in `changed_fields`.
- Role and site changes re-read the target and re-check the permission inside the transaction,
  under the admin-change lock; nobody is left without a role (employee and manager need a site);
  people are only added to active sites.

## 5. Location control

- Read once at start/stop: `getCurrentPosition({ enableHighAccuracy: true, timeout: 10000, maximumAge: 0 })`.
  No tracking in between.
- The server computes the haversine distance to each site the person is linked to and sets
  `zone_status`: `in_zone` | `out_of_zone` | `uncertain` (accuracy worse than threshold) |
  `no_location` (denied or timeout).
- Default policy is **flag, not block**: the event is always stored, the employee sees a
  plain-language warning, the manager sees the flag. A `block` policy per site is possible later.
- The server timestamp is authoritative; the client timestamp is kept to detect offline delay.
- Known limit: browser location can be spoofed. Combined with manager review, clocking from home does
  not go unnoticed, which is the goal.
- GDPR: one explanation screen before the browser prompt; coordinates are encrypted (§6) and purged
  after a configurable period (default 31 days), keeping only `site_id` + `zone_status`.

## 6. Encryption of personal data

Goal: a leaked database dump or backup, or an SQL-injection read, yields no usable personal data.

- **What is encrypted:** names, e-mail (if any), free-text notes, coordinates (lat/lng/accuracy),
  and any future personal field (correction reasons, messages, attests).
- **What stays plaintext:** random ids, timestamps, enum values (`kind`, `zone_status`), site ids.
  Without the key these only say "some id started at 07:42 at site 3".
- **Algorithm:** XChaCha20-Poly1305 per field. Stored as `bytea`:
  `key_version (1 byte) || nonce (24 bytes) || ciphertext+tag`.
  Associated data = `table || column || row id`, so a ciphertext cannot be moved to another row or column.
- **Key:** 256-bit data key loaded at startup from a file or environment variable, never stored in the
  database or in the same backup as the database. Multiple key versions are supported for rotation
  (`admin rotate-key` re-encrypts in batches).
- **Lookups:** names are never queried in SQL. Lists are decrypted in the app and sorted in memory
  (fine for hundreds of people). If an exact lookup is ever needed, a blind index
  (HMAC-SHA256 with a separate key) is added for that field.
- **Type safety:** an `Encrypted<T>` wrapper type; repository functions only accept or return
  decrypted values through a `Crypto` service, so plaintext cannot be written by accident.
- **Honest limits (document in README):** this protects against database and backup leaks, not
  against a full compromise of the running app server (the key is in its memory). Losing the key
  means losing the data: operators must back up the key separately.
- Logs never contain decrypted personal data.

## 7. PWA ("real app" install)

- `manifest.webmanifest`: `id`, `name`, `short_name`, `start_url`, `scope`, `display: "standalone"`,
  192/512 px icons + `maskable`, `theme_color`, `background_color`, `lang`.
- Service worker: cache app shell + WASM; network-first for server functions.
- Android: Chrome installs a WebAPK; capture `beforeinstallprompt` → own "Installeer de app" button.
- iOS: no install prompt; detect Safari + not standalone → 3-step "Deel → Zet op beginscherm" screen.
- Offline clocking: action queued in IndexedDB with client timestamp + location, sent on reconnect;
  UI shows "Bewaard, wordt verstuurd".
- **Passkeys are bound to the domain (RP ID).** The domain must be final before real enrollment;
  changing it invalidates all passkeys. Documented prominently for operators.

## 8. Screens

Employee: E1 welcome/install, E1b iPhone steps, E2 create passkey, E3 location explanation, E4 START,
E5 started/STOP, E6 out of zone, E7 day summary, E8 passkey login.
Manager: M1 Mijn team (add employee, link device, status today), M2 QR code (countdown, switches to
"Gekoppeld").
Admin: A0 organisation settings (name, short name, colour, logo, time zone), A1 sites, A2 people
and roles (assign manager to sites).
Setup: S1–S6 first-run wizard (§2.1).
UI strings in one i18n module: nl-BE first, fr-BE and en next.

## 9. Data model (version 1)

Encrypted columns marked 🔒.

- `organisation_settings` — single row: display name, short name, brand colour, logo (bytea, nullable),
  time zone, retention days, zone defaults, setup_completed_at
- `setup_code` — code_hash, created_at, used_at (only while in setup mode)
- `site` — id, name, address 🔒, lat, lng, radius_m, break_minutes, zone_policy, active, cost_centre (plain text)
  *(site location is not personal data and is needed in plaintext for the zone check)*
- `person` — id, webauthn_user_handle, display_name 🔒, email 🔒, payroll_number 🔒 (+ blind index
  `payroll_number_index`, unique when present, for API lookups), active, created_by, created_at
- `role_assignment` — person_id, role, site_id (nullable), created_by, created_at
- `enrollment_token` — token_hash, person_id, issued_by, expires_at, used_at
- `passkey` — id, person_id, credential (JSONB), label (coarse device description from the User-Agent, max 60 chars), created_at, last_used_at, revoked_at, revoked_by
- `session` — managed by the `tower-sessions` store
- `time_event` — id, person_id, kind (start|stop), server_ts, client_ts, site_id, location 🔒
  (lat, lng, accuracy as one encrypted blob, nullable after purge), zone_status, source, created_at
  — **append-only**
- `audit_log` — id, actor_id, action, entity, entity_id, changed_fields, at (no plaintext personal data)
- `work_schedule` — person_id, weekday (1 = Monday .. 7), minutes: contract minutes per weekday
- `correction` — id, time_event_id | target_correction_id | neither, person_id, kind, corrected_ts,
  reason 🔒, created_by, created_at, voids_event — **append-only**. A correction replaces the time of an
  event, voids it, or (no target) adds a missing event; an addition can itself be replaced or voided
  through `target_correction_id`. The latest correction per target wins; `time_event` is never touched.
  A correction keeps its target on the same local day. Audit rows never contain the reason.
- `period_validation` — id, site_id, period_start, period_end (inclusive), validated_by, validated_at,
  reopened_by, reopened_at. Rows are never deleted; reopening only fills the `reopened_*` columns
  (trigger). The repository refuses overlapping active validations of a site (per-site advisory lock).
  Only finished days (strictly before today in the organisation time zone) can be validated. A day
  is closed when ANY site that can lock it is validated for it: the sites of that day's events plus
  the person's current employee sites, so moving a person to another school (API, role change)
  never reopens a validated day. Corrections on a closed day, and clock taps (defence in depth),
  are refused until HR or an admin reopens it. `correct()` takes the same per-site advisory locks
  as `validate_period` (in site order) before checking, in one transaction, so a correction cannot
  commit into a period validated at the same moment. A database trigger also refuses a new active
  validation that overlaps an active one of the same site. A manager cannot correct their own
  hours (HR and admins can). Night shifts crossing midnight are split per local day (known
  limitation): each day is computed, corrected and validated on its own.
- Reserved for next steps: `contract_type`, `export_batch`, `message`.

## 10. Repository layout

```
Cargo.toml            workspace
crates/domain/        pure logic: zone calc, day totals, break deduction (no IO)
crates/crypto/        field encryption, Encrypted<T>, key loading and rotation
crates/theme/         Material 3 colour roles and CSS from a seed colour (no IO)
crates/app/           Dioxus fullstack app (features: web, server)
  src/client/         screens, components, PWA glue, geolocation, WebAuthn glue
  src/server/         server functions, passkey auth, sessions, roles, repository (sqlx), audit
crates/admin/         CLI: bootstrap first admin, seed sites from TOML, purge coordinates, rotate key
migrations/           sqlx migrations
config/example/       example settings and sites.toml
public/               manifest, icons, service worker
tests/e2e/            browser tests (virtual WebAuthn authenticator, geolocation override)
deploy/               Dockerfile, compose file, operator docs
```

## 11. Testing

- `domain` and `crypto`: unit tests (zone edges, accuracy threshold, day totals, break deduction,
  encrypt/decrypt round trip, wrong associated data fails, key rotation).
- Server: integration tests against a throwaway PostgreSQL database (`sqlx::test`), including
  "no plaintext personal data in any table" checks.
- Passkeys: server tests with a software authenticator; browser tests with Chromium's virtual
  authenticator.
- Location: browser tests with geolocation override (in zone, out of zone, denied).
- Manual device test on a real iPhone and an older Android, installed as PWA, before a pilot.
- TDD: every change starts with a failing test.

## 12. Build tasks

| # | Task | Depends on |
|---|---|---|
| T0 | Scaffold workspace (domain, crypto, app, admin), Dioxus fullstack hello world | — |
| T1 | `domain`: haversine, zone status, day totals with break deduction | T0 |
| T2 | `crypto`: XChaCha20-Poly1305 field encryption, `Encrypted<T>`, key loading, key versions | T0 |
| T3 | DB schema + migrations (§9) | T0 |
| T4 | Sessions, auth extractors, role/scope checks | T3 |
| T5 | Enrollment tokens + passkey registration and login | T2, T4 |
| T6 | WebAuthn + geolocation browser glue | T0 |
| T7 | `admin` CLI: bootstrap first admin (prints QR), seed sites, purge coordinates, rotate key | T2, T3, T5 |
| T8 | Clock server function: auth, zone check, encrypted location, append event, audit | T1, T2, T4 |
| T9 | Employee screens E1–E8 | T5, T6, T8 |
| T10 | Manager screens M1–M2 (team, add/deactivate, QR enrollment with polling, revoke, today status) | T5 |
| T11 | Admin screens A1–A2 (sites, people and roles) | T4 |
| T12 | PWA: manifest, icons, service worker, install flow, offline queue | T9 |
| T13 | Browser tests | T9–T12 |
| T14 | Container image, compose file, operator docs (domain, key backup, database backups) | T12 |
| T15 | First-run setup: setup mode + setup code, wizard S1–S6, A0 organisation settings, branding (name, colour, logo) applied in header, manifest and theme colour | T5, T9 |

Security-relevant tasks (T2, T4, T5, T8) get an independent review before merge.

## 13. Open points

| # | Item | Needed for |
|---|---|---|
| 1 | Final domain with HTTPS for the first deployment (passkey RP ID) | real enrollment |
| 2 | Site list for the first deployment (name, address, coordinates, radius, break length) | pilot |
| 3 | Hosting and backup responsibility for the first deployment | pilot |
| 4 | Permission to publish, if the work is done for an employer | first public push |
| 5 | Project name and branding | first public push |
| 6 | People without a smartphone: kiosk mode later or out of scope | later |

## 14. Payroll export and integrations (decided 2026-11-24; server side and screens built, connectors to do)

Depends on Uren (hours overview), corrections and validation. The backend of these exists
(`server::hours`, `api::hours`: overview, day detail, corrections, period validation, contract
minutes, payroll number, site cost centre; schema in §9); the screens and the export are still to do.

- **Built (server)**: `server::export` + `api::export` (HR/admin only; custom profiles admin only; managers
  have no export for now), tables `export_profile` and `export_batch` (append-only, no file and no
  personal data stored; regenerating from the same inputs gives the same checksum, `export_verify`),
  API v1 `GET /hours`, `GET /periods`, `GET /exports/{profile}`, `PUT /people/{external_id}`
  (see `docs/api.md`). Rules: only days covered by an active period validation of the person's site
  are exported (otherwise `Blocked` with the unvalidated stretches per site, no partial export);
  people with hours but no payroll number block the export and are listed by name in the staff UI
  (the API only answers `409`); the person key is the payroll number, `site_code` the cost centre of
  the site where the day was worked (site of the day's first event; contract days without events
  fall under the person's first selected site), and that site must have validated the day; the time
  basis can override the profile's. Security hardening (2026-11-24): only days before today are
  exported (a period ending today or later is cut at yesterday and recorded so; never an
  "unvalidated" problem); CSV text cells and headers starting with `= + - @ TAB CR` get a leading
  `'` (plain numbers excepted; XLSX writes strings); unencodable windows-1252 values are a
  problem naming the persons, not a failed export; limits: 200 000 rows per export, two exports
  generated at a time, 10 API exports per minute per key (`docs/export-profiles.md`).
- **Neutral export model**: per person per day from validated periods: payroll/contract number
  (encrypted), site code (cost centre), date, start, stop, break, worked minutes, contract minutes,
  registered-vs-contract mode, later absence codes.
- **Export profiles** (declarative TOML, no code): format (CSV/XLSX), separator, encoding, decimal sign,
  columns and headers, date/duration formats, code mapping, per-day vs per-period. Shipped: generic CSV,
  generic Excel, Acerta (see `docs/export-acerta.md`). Admins can copy and adapt a profile. An
  `Exporter` trait covers formats a profile cannot express and outbound connectors.
- **Export batches** are recorded (who, when, period, profile, checksum); only validated periods export.
- **Public REST API v1** under `/api/v1`, separate from the browser server functions:
  - API keys managed in Instellingen › Integraties: name, scopes (`people:read`, `people:write`,
    `hours:read`, `export:read`), optional site restriction, expiry; shown once, stored hashed,
    last-used, revocable; `Authorization: Bearer`; per-key rate limit; every call audited; optional IP
    allowlist. Keys grant access to decrypted personal data and are treated accordingly.
  - `people:write` only creates and changes **employees** (never manager, HR or admin), only
    people who already have role assignments at the key's sites (no claiming of role-less people);
    answers for people it may not touch are identical (`409`); the Integraties page warns when the
    scope is chosen. Restricted keys see only roles, sites, days and events of their own sites.
    Mutating calls write their `api.call` audit row in the same transaction; failed
    authentications are audited with a keyed client hash under a per-client rate limit.
  - Endpoints: `GET /people`, `PUT /people/{external_id}` (HR sync upsert), `GET /hours`,
    `GET /exports/{profile}`, `GET /periods`. Webhooks later (e.g. period validated).
  - **OpenAPI** generated from code with `utoipa` (+ `utoipa-axum`), served at `/api/v1/openapi.json`;
    interactive docs with **Scalar** via `utoipa-scalar` at `/api/v1/docs` (owner's choice over
    RapiDoc). The Scalar script is vendored in `crates/app/vendor/scalar/` and embedded in the binary (no CDN) so the strict CSP stays intact; the
    docs page is **off by default** (`API_DOCS=on`), has its own CSP (no inline script) and Scalar is
    configured without agent, MCP, developer tools, client button, telemetry, fonts and proxy.
- **Outbound connectors**: "send directly" next to "download" (e.g. Acerta REST API if access is
  granted); connector credentials stored encrypted with the data key.

- **Built (screens)**: Uren › Export (HR/admin: profile, period up to 366 days, schools, basis, preview with
  plain-language problems, download from base64 via Blob, history with verify), Instellingen › Export
  (admin: copy shipped profile, TOML editor with server-side validation, deactivate) and
  Instellingen › Integraties (admin: API keys shown once, revoke, links to `/api/v1/docs`). Staff
  pages show all times in the organisation time zone (`StaffContext.timezone`), not the browser's.

## v1.1: clock note

After a start or stop that was not in the zone (`out_of_zone`, `uncertain`, `no_location`), the
employee may add one short explanation (e.g. “uitstap met de klas”). It is optional and never
blocks or delays clocking: the event is already stored when the warning screen shows the field,
and "OK" skips it.

- **Data model**: `clock_note` (`id`, `event_id` unique → `time_event`, `person_id` → `person`,
  `note_enc`, `created_at`). The note is free text, so it is encrypted with associated data
  `clock_note || note || id`. A composite foreign key `(event_id, person_id)` → `time_event (id,
  person_id)` ties the note to the person of the event. Insert-only: triggers refuse update,
  delete and truncate. Like correction reasons and locations, notes are not part of key rotation
  (append-only); keep older key versions.
- **Rules (server)**: one note per event, no edits; only the event's own person; only when the
  event's zone status is not `in_zone`; only on the same local day in the organisation time
  zone; 3 to 500 characters once trimmed (the same rule as correction reasons,
  `domain::free_text`). `ClockResult.event_id` names the new event; `add_clock_note(event_id,
  text)` adds the note. No audit row: it is the employee's own action, not a manager mutation.
- **Staff**: the note shows under the zone flag in the Uren day detail and under the name of a
  flagged start on "Vandaag", decrypted only for staff whose scope covers that person (the same
  checks as the rest of those pages). Not in the payroll export.
