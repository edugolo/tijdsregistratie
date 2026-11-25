# Operations

Operator guide for the `admin` CLI. All commands read their configuration from environment variables:

| Variable | Used by | Meaning |
| --- | --- | --- |
| `DATABASE_URL` | all except `generate-key` | PostgreSQL connection string |
| `DATA_KEY_FILE` | all except `generate-key`, `purge-locations` | file with the versioned data keys |
| `BLIND_INDEX_KEY_FILE` | `bootstrap-admin` | file with the single blind-index key |
| `APP_ORIGIN` | `bootstrap-admin` | public origin, e.g. `https://tijd.example.org` |

Every command except `generate-key` applies pending database migrations first.

## First setup

1. **Generate both key files** with mode 0600 (the `umask` makes the file private from the start):

   ```sh
   (umask 077; admin generate-key --version 1 > /etc/tijd/data.key)
   (umask 077; admin generate-key --version 1 > /etc/tijd/blind.key)
   ```

   The data key file holds one `vN:<base64>` line per key version; the blind-index key file holds
   exactly one line. Check with `ls -l` that neither file is readable by other users.

2. **Back up the keys separately from the database.** A database dump must never be stored together
   with the key files, otherwise the encryption protects nothing. Keep the key backup offline or in a
   separate secret store.

   **Losing the data key means losing all personal data** (names, e-mail addresses, site addresses,
   coordinates). There is no recovery.

3. **Bootstrap the first admin:**

   ```sh
   admin bootstrap-admin --name "Voorbeeld Beheerder"
   ```

   This creates the person with the admin role, prints the enrollment link
   (`$APP_ORIGIN/koppel#<token>`, valid 30 minutes, single use) and shows it as a QR code.
   Scan it with the admin's own phone to register a passkey. If the token expires, run
   `admin list-admins` to find the person; issuing a new link currently means running
   `bootstrap-admin --force` again. The command refuses when an active admin already exists unless
   `--force` is given.

4. **Seed the sites** from a TOML file (see `config/example/sites.toml`):

   ```sh
   admin seed-sites --file sites.toml
   ```

   Sites are matched by exact name: existing ones are updated, new ones are created. Running it again
   with the same file changes nothing. Addresses are stored encrypted.

## Purging coordinates

Coordinates of clock events are kept for `location_retention_days` (default 31) and are then removed;
`site_id` and `zone_status` stay.

The server purges by itself: about a minute after startup and then every 24 hours it removes
coordinates older than the retention period. No cron entry is needed. A lock in the database makes
sure only one instance runs it at a time, a failed run is logged as a warning and retried at the next
interval, and the info log only holds the number of purged locations and the cutoff. Admins see the
last run under Instellingen > Organisatie; it is stored in `organisation_settings`
(`last_location_purge_at`, `last_location_purge_count`).

To purge by hand, for example right after lowering the retention period:

```sh
admin purge-locations
```

`--before <RFC3339>` overrides the cutoff. The command prints the number of purged locations and
records the run like the automatic purge does.

## Key rotation

1. Back up the current key file.
2. Append a new line with the next version to the data key file. Keep the old lines:

   ```sh
   admin generate-key --version 2 >> /etc/tijd/data.key
   ```

3. Restart the app so it encrypts new data with the new key.
4. Run `admin rotate-key`. It re-encrypts `person.display_name`, `person.email` and `site.address`
   that are not on the newest key version, in batches inside transactions, and prints the counts.
   It can be repeated safely.

**`time_event.location_enc` is not rotated.** The `time_event` table is append-only (a database
trigger only allows purging the location), and locations are removed after the retention period
anyway. Therefore **keep old key versions in the key file until every location written with them has
been purged**: at least `location_retention_days` after step 3, then run `purge-locations` and only
then remove the old lines. Removing a version earlier makes the locations written with it unreadable.

The blind-index key is not rotated by this command.

## Setup never re-opens, and recovery

The first-run wizard is available only until the first admin has registered a passkey. The
database records this in `organisation_settings.setup_completed_at` (set by a trigger at the
first passkey of an admin, backfilled by a migration for existing installations, and never cleared
again). From then on `setup_required` is false forever: no setup code is announced in the log or
accepted, and `complete_setup` is refused, whatever happens to the admins afterwards (lost
devices, revoked passkeys, deactivated or deleted admins). An existing organisation can never be
overwritten through the wizard.

**If every admin has lost their devices**, the only way back in is the operator CLI, which needs
access to the database and the key files:

```sh
admin bootstrap-admin --name "Voorbeeld Beheerder" --force
```

It creates a new admin and prints an enrollment link (valid 30 minutes, single use).
`--force` is needed because active admins still exist. Deactivate the old admins afterwards if they
should no longer have access. Before setup is completed, the setup code is logged at startup (or
shown with `admin setup-code`).

## Other commands

- `admin list-admins` prints id, state and name of every admin (operator tool; names are decrypted).
- `admin generate-key [--version N]` prints one key line to stdout; it never touches the database.

## Database hardening

The migrations make the data hard to alter by accident or by a compromised app:

- `time_event` rejects `UPDATE` (except nulling `location_enc` for the coordinate purge), `DELETE`
  and `TRUNCATE`.
- `audit_log` is append-only: `UPDATE`, `DELETE` and `TRUNCATE` are rejected.

These are triggers and a **safety net, not a security boundary**: unless the app connects with a
role that does not own the tables, anyone with the app's database credentials can disable them.
**The table owner (and any superuser) can still disable or drop them**
(`ALTER TABLE ... DISABLE TRIGGER`), so they protect against bugs and against an attacker who only
has the app's credentials, not against a malicious database owner. To make that real, run the app
with a separate least-privilege role and keep the owner role for migrations only. **Doing this is
recommended for every production deployment**; the single-role setup of `deploy/compose.yaml`
(the database user owns the schema) is only the quick start:

```sql
-- Once, as a superuser. The migration role owns the schema; the app role owns nothing.
CREATE ROLE tijd_migrate LOGIN PASSWORD '...';   -- runs migrations / the admin CLI
CREATE ROLE tijd_app     LOGIN PASSWORD '...';   -- the web app at runtime
ALTER DATABASE tijd OWNER TO tijd_migrate;

-- After every migration run, as tijd_migrate:
GRANT USAGE ON SCHEMA public TO tijd_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO tijd_app;
REVOKE UPDATE, DELETE, TRUNCATE ON time_event, audit_log FROM tijd_app;
GRANT UPDATE (location_enc) ON time_event TO tijd_app;   -- coordinate purge only
REVOKE TRUNCATE, TRIGGER, REFERENCES ON ALL TABLES IN SCHEMA public FROM tijd_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO tijd_app;
-- No CREATE on the schema, no ownership, no DDL for tijd_app.
```

Point `DATABASE_URL` of the app at `tijd_app`; run `admin` commands that migrate (every command except
`generate-key`) with `tijd_migrate`. `admin purge-locations` needs the `UPDATE (location_enc)` grant and
update access to `organisation_settings` (to record the run), so it can run as `tijd_app`. The session and audit tables the app writes keep their
insert/update grants. Re-run the `GRANT`/`REVOKE` block after each migration that adds tables.

## Stale open days

A person who starts and never stops leaves an open day. `my_today` reports it (`stale_open_since`)
and the team overview shows "stop missing" for that person; a new tap on the next day still starts a
new day. The forgotten stop is corrected by a manager later; the correction module (new rows that
reference the original, with a reason) is not built yet, so until then it stays visible but
uncorrected.

## Erasing a person

**Erasing a person is not supported yet.** Time events are append-only by design (triggers forbid
`DELETE`), and other rows reference the person. For GDPR this means a retention policy and a
pseudonymisation plan still have to be designed and agreed (for example: after the retention
period, replace the encrypted name and e-mail by a tombstone and keep the anonymous events).
Until then, deactivate the person instead (`active = false`); this keeps the history but blocks
sign-in. Do not delete rows by hand: it breaks the audit trail and will be refused by the triggers.

## Reverse proxy and rate limiting

Run the app behind a reverse proxy (Traefik, nginx, Caddy) that terminates TLS and **rate limits** the
public endpoints, in particular the login and enrollment routes (`/koppel`, the passkey `/api/...`
functions) and `/api/clock`. A limit of a few requests per second per IP with a small burst is
enough for this application. Only let the app trust forwarded client-IP headers when the proxy is the only way to reach it.

The enrollment token sits in the URL fragment (`/koppel#<token>`), which browsers never send to
servers, so it does not appear in proxy logs or `Referer` headers. Old `/koppel/<token>` links
show "Deze code werkt niet meer."

The app itself limits the sign-in and enrollment endpoints (`/api/auth/*`, except the session
probe) per client address (`AUTH_RATE_LIMIT_PER_MINUTE`, default 60: schools sit behind one NAT).
Failed `login/finish` and `register/finish` attempts (HTTP 400/401) are counted per client
separately: 10 failures in 10 minutes block further finish attempts from that client. The setup
wizard endpoints (`/api/setup/*`) allow 10 requests per minute per client, and the public reads
`/api/branding` and `/api/setup/required` 120. Clocking (`POST /api/clock`) is limited to 20 per
minute per signed-in person, whatever the address. The client address is the
TCP peer address. Behind a reverse proxy that is the proxy's address, so **set
`TRUST_PROXY_HEADERS=true`** (only when the proxy is the sole way in): the app then uses the last
entry of the last `X-Forwarded-For` header line, which the proxy adds (IPv4-mapped IPv6 addresses
count as IPv4, IPv6 clients are grouped by /64), so clients are told apart again. Without it, all
clients behind the proxy share one bucket and can lock each other out (for example at 08:00).
If no client address is available at all, per-client limiting is off, only a global cap of 600
requests per minute applies and a warning is logged.

## Address search (geocoder)

The school editor (Instellingen > Scholen) and the setup wizard (step 3) can look up an address
and fill in its coordinates. The **server** sends the search text to a Nominatim-compatible
`/search` endpoint (OpenStreetMap open data); browsers never contact it, so the Content Security
Policy stays `connect-src 'self'`. Only admins can search, and during first-run setup only a
session that already entered a valid setup code. A school address is not personal data, but it
does leave your server, so tell your data protection officer if you keep this on.

- `GEOCODER_URL` is the geocoder's `/search` URL. Unset: the public instance
  `https://nominatim.openstreetmap.org/search`. Empty or `off`: the search is switched off and the
  forms only show the manual fields and "Gebruik mijn huidige locatie".
- The app follows the public instance's usage policy: one request per second for the whole
  server (further searches wait briefly, then are refused), identical searches are cached in
  memory for 24 hours, requests time out after 5 seconds and carry the User-Agent
  `Tijdsregistratie/<version> (+https://github.com/edugolo/tijdsregistratie)`. The public instance
  is meant for light use only. A school group that sets up a handful of schools is fine.
- For heavier or guaranteed use, host your own Nominatim (or another service that answers the
  Nominatim `jsonv2` format) and set `GEOCODER_URL` to it, for example
  `http://nominatim.internal:8080/search`. Photon is not compatible as is: its answers have a
  different format.
- Search text is never logged. Failures show the user a generic message.
- `/api/geocode` is rate limited to 30 requests per minute and client address, on top of the
  global one request per second upstream.

## Key file permissions and blob format

The app logs a warning at startup, and the admin CLI prints one on stderr, when `DATA_KEY_FILE` or
`BLIND_INDEX_KEY_FILE` is readable or writable by group or others (use `chmod 600`).

- Key files must be readable by the app user only (`chmod 600`). The `crypto` crate provides
  `check_permissions(path)`, which returns a warning when group or others can access the file; the
  app and the admin CLI log it at startup.
- The key version byte of every encrypted blob is part of the AEAD associated data. Blobs written
  before this change cannot be read; there was no production data when it was introduced. A database
  that already holds encrypted data from an older build has to be re-created.

## Deployment

The `deploy/` directory holds a multi-stage `Dockerfile` and a `compose.yaml` (app and PostgreSQL);
the README has the quick start. This section covers running it for real.

### Reverse proxy and TLS

Passkeys require HTTPS (except on `http://localhost`). Terminate TLS in a reverse proxy and forward
to the app, which the compose file publishes on `127.0.0.1:${HOST_PORT}` only.

- Set `APP_ORIGIN` to the final public origin (`https://...`, no path) **before first use**.
  Passkeys are bound to that domain: changing it invalidates every registered passkey.
- Keep `COOKIE_SECURE=true` (the default) for an `https` origin.
- Set `TRUST_PROXY_HEADERS=true` only when the proxy overwrites `X-Forwarded-For` and is the only
  way to reach the app. Otherwise clients can spoof their address and bypass the per-address rate
  limit on `/api/auth/*`.
- Forward the `Host` header unchanged and enable WebSocket/HTTP streaming as needed by your proxy.
- Optional hardening: restrict `/api/health` to the local network or the proxy.

### Backups

- **Database:** `pg_dump` the database on a schedule, for example
  `docker compose -f deploy/compose.yaml exec -T db pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB" | gzip > tijd-$(date +%F).sql.gz`.
  Test a restore regularly.
- **Keys:** back up `secrets/` (both key files) **separately** from the database dumps, offline or in
  a separate secret store. Restoring a dump without the matching key leaves all personal data
  unreadable, and storing both together makes the encryption pointless.
- Back up again after every key rotation (the data key file gains a line).

### Upgrades and migrations

```sh
git pull
docker compose -f deploy/compose.yaml --env-file .env up -d --build --wait
```

Pending database migrations are applied automatically at startup (and by every `admin` command
except `generate-key`). Take a database backup first. The old container keeps serving until the new
one is built; there is a short gap while it restarts.

### Running the admin CLI

The image contains the `admin` binary; run it with the same environment and key files as the app:

```sh
docker compose -f deploy/compose.yaml --env-file .env run --rm app admin bootstrap-admin --name "Example Admin"
docker compose -f deploy/compose.yaml --env-file .env run --rm app admin setup-code
```

The container user must be able to read the key files (see `APP_UID` / `APP_GID` in
`config/example/.env.example`; rootless Docker needs `0`/`0`). To seed sites, mount the file:
`run --rm -v "$PWD/sites.toml:/tmp/sites.toml:ro" app admin seed-sites --file /tmp/sites.toml`.

### Purge of coordinates

The app purges coordinates daily by itself (see "Purging coordinates" above); there is nothing to
schedule. For a manual run: `docker compose -f deploy/compose.yaml --env-file .env run --rm -T app admin purge-locations`.
