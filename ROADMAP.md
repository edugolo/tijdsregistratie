# Roadmap

The product is not finished yet; versions stay below 1.0 until it is. Items are not ordered by
priority. Details of what shipped live in `docs/plan.md`.

## Done in v0.0.1

- **Employee explanation for an out-of-zone start or stop.** Optional, encrypted, same day only, not
  after validation; shown to staff in scope on Vandaag and in the Uren day detail. Purged with the
  coordinates.
- **Automatic retention purge.** The server removes coordinates and clock notes after
  `location_retention_days` (default 31), daily, under an advisory lock; admins see the last run.
- **Typed enrollment code.** A 12-symbol code and a copy-link button next to the QR code, for devices
  that cannot scan it. Rate limited, stored as an HMAC, audited as link or code.
- **Clear export feedback.** Downloaden says why it is disabled, refuses zero-row exports and names
  the file and row count on success.

## Planned

### Acerta payroll profile

The shipped `acerta-kalender` profile is an inferred layout with a placeholder looncode. Needed from
Acerta or payroll: the import definition (separator, column order, date format) and the employer's
looncodes for presence and absences. Then add a custom profile through the app's profile screen.

### Map of the registration location

Show staff where a flagged registration happened, while the coordinates still exist (31 days).
Options, from most to least private:

1. Distance and direction only ("340 m north of the school"), drawn as SVG: no third parties, fits
   the current Content-Security-Policy (`img-src 'self'`).
2. Map tiles proxied and cached by the server, with OpenStreetMap attribution: a real map without the
   browser contacting a third party; respects the OSM tile usage policy only with caching.
3. A link that opens the coordinates on openstreetmap.org: cheapest, but sends the coordinates to a
   third party when used.

### Open decisions

- Whether clock notes belong in the payroll export.
- Whether typed enrollment codes should be limited to staff devices.
