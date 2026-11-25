# Roadmap

Planned work that is not in v1 yet. Items are not ordered by priority.

## Employee justification for an out-of-zone start

Today an out-of-zone, unknown or inaccurate location is stored and flagged for the manager, but the
employee cannot say why (an outing, working from home, a failing GPS). The manager has to ask and can
only record the reason afterwards, as a correction on the Uren page.

- After a start or stop with zone status `out_of_zone`, `uncertain` or `no_location`, offer an
  optional one-sentence explanation (3 to 500 characters, same limits as a correction reason).
- Never block or delay the registration for it (see DESIGN.md, principle 4); the field is optional.
- Store it encrypted with the data key, like the correction reason; keep it out of the audit log.
- Show it next to the flag in "Mijn team" and in the day detail of the Uren page.
- Decide whether it belongs in the payroll export.

## Scheduled purge of coordinates

Coordinates are kept for `location_retention_days` (default 31) and then removed by
`admin purge-locations`. Nothing runs that command by itself, so a deployment without a scheduled job
keeps the coordinates indefinitely.

- Run `admin purge-locations` daily (docs/operations.md, "Scheduled purge of coordinates").
- For the Dokploy deployment: a schedule on the application, daily at 03:15, running the command in
  the app container.
- Make the purge visible: log the number of purged rows and show the date of the last purge to admins.
- Consider running the purge inside the server (a daily background task) so a deployment cannot forget it.

## Typed enrollment code for devices that cannot scan a QR code

Linking an additional device (`start_own_enrollment`, and a manager enrolling someone) only offers a
QR code. A laptop without a usable camera cannot scan it, which makes registering an admin's laptop
needlessly awkward. The enrollment token is 32 random bytes, far too long to type; the issued
`EnrollmentDto` already carries a `url`, but the screen only shows the QR code.

- Offer a short unique code next to the QR code, typed on the `/koppel` page of the device to link.
  Use the unambiguous 32-symbol alphabet of the setup code (no 0/O, 1/I), grouped for reading
  (for example `ABCD-EFGH-JKLM`, 60 bits).
- Same rules as the QR token: single use, expires with the token, stored only as a hash, issuing a new
  one invalidates the old one, passkey re-check before staff get one for themselves.
- Typing makes guessing the attack, so rate-limit attempts per client address and globally, and lock
  the code after a few wrong tries.
- Also show the enrollment link with a copy button, as the simplest way to move it to a laptop.
- Audit row for issuing and using a code, like the existing `enrollment.issued`.
- Decide whether employees get typed codes too, or only staff (admins, managers, HR).

## Export screen: make "Downloaden" and its feedback clear

Reported while test-clicking on the deployed app: clicking "Downloaden" shows no feedback and nothing
seems to happen.

Causes (crates/app/src/client/staff/export.rs unless noted):

- The download is disabled whenever the current choices differ from the last preview (`stale`, line
  118; `can_download`, line 119; `disabled`, line 340). There is no `.st-btn:disabled` style in
  `crates/app/assets/staff.css` (only `.st-filter:disabled`), so the disabled button looks active and a
  click is swallowed silently.
- The only hint is a small grey line far below the buttons ("Je keuzes zijn veranderd. Maak opnieuw een
  voorbeeld.", line 355).
- The green "Bestand gemaakt" notice (line 348) stays after the choices change, so it reads as the
  result of the current click. `done` is reset only when a new preview or download starts
  (lines 126 and 154), not when the selection changes.
- A download with zero rows is allowed and reported as success, giving an empty file (the history
  shows a row with 0 Rijen). `can_download` does not look at `row_count`.

Direction:

- Style `.st-btn:disabled` (dimmed, no pointer cursor) and say why next to the button, for example
  "Maak eerst een voorbeeld van deze keuzes".
- Clear `done` as soon as the selection changes, and name the file in the success notice
  ("Bestand 'x.csv' gedownload, n rijen").
- Offer to download from a fresh selection in one step (make the preview implicitly), or enable
  Downloaden only after a preview and move the hint under the buttons.
- Block or warn on zero rows ("Er zijn geen gevalideerde uren in deze periode").
- Show "Even geduld" as a visible state on the button while the file is made.
