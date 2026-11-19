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
   (`$APP_ORIGIN/koppel/<token>`, valid 30 minutes, single use) and shows it as a QR code.
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
`site_id` and `zone_status` stay. Run the purge daily, for example from cron:

```cron
15 3 * * * . /etc/tijd/env && /usr/local/bin/admin purge-locations >> /var/log/tijd-purge.log 2>&1
```

`--before <RFC3339>` overrides the cutoff. The command prints the number of purged locations.

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

## Other commands

- `admin list-admins` prints id, state and name of every admin (operator tool; names are decrypted).
- `admin generate-key [--version N]` prints one key line to stdout; it never touches the database.

## Database hardening

The migrations make the data hard to alter by accident or by a compromised app:

- `time_event` rejects `UPDATE` (except nulling `location_enc` for the coordinate purge), `DELETE`
  and `TRUNCATE`.
- `audit_log` is append-only: `UPDATE`, `DELETE` and `TRUNCATE` are rejected.

These are triggers. **The table owner (and any superuser) can still disable or drop them**
(`ALTER TABLE ... DISABLE TRIGGER`), so they protect against bugs and against an attacker who only
has the app's credentials, not against a malicious database owner. To make that real, run the app
with a separate least-privilege role and keep the owner role for migrations only:

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
`generate-key`) with `tijd_migrate`. `admin purge-locations` needs only the `UPDATE (location_enc)`
grant, so it can run as `tijd_app`. The session and audit tables the app writes keep their
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
public endpoints, in particular the login and enrollment routes (`/koppel/*`, the passkey `/api/...`
functions) and `/api/clock`. A limit of a few requests per second per IP with a small burst is
enough for this application. Only let the app trust forwarded client-IP headers when the proxy is the only way to reach it.

## Key file permissions and blob format

- Key files must be readable by the app user only (`chmod 600`). The `crypto` crate provides
  `check_permissions(path)`, which returns a warning when group or others can access the file; the
  app and the admin CLI are meant to log it at startup (not wired in yet).
- The key version byte of every encrypted blob is part of the AEAD associated data. Blobs written
  before this change cannot be read; there was no production data when it was introduced. A database
  that already holds encrypted data from an older build has to be re-created.
