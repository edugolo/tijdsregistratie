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
