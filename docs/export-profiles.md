# Export profiles

The `export` crate turns neutral rows (one per person per day per kind) into a CSV or XLSX file
described by a declarative TOML **profile**. No code is needed to adapt a profile. Shipped profiles
live in `config/export-profiles/`; copy one and edit it.

Output is deterministic: rows are ordered by person key, date, kind and site, regardless of input
order, and every export has a SHA-256 checksum of its bytes.

## Writing a profile

```toml
profile_version = 1          # required, must be 1
name = "My profile"          # required
description = "..."          # optional
format = "csv"               # csv | xlsx
```

Unknown keys, unknown fields and invalid values are rejected with a clear message; all validation
problems are listed together.

### Options

| Key | Values | Default | Notes |
| --- | --- | --- | --- |
| `format` | `csv`, `xlsx` | required | |
| `encoding` | `utf-8`, `windows-1252` | `utf-8` | csv only. A character that cannot be encoded fails the export. |
| `bom` | bool | `false` | utf-8 byte order mark (helps Excel open UTF-8 CSV). |
| `separator` | one ASCII char | `;` | csv only. Not a quote or newline; must differ from `decimal_separator`. |
| `quote` | `necessary`, `always` | `necessary` | csv only. |
| `line_ending` | `crlf`, `lf` | `crlf` | csv only. |
| `decimal_separator` | `,` `.` | `,` | csv only; xlsx stores real numbers. |
| `header` | bool | `true` | Write the header row. |
| `sheet_name` | text, max 31 chars | `Export` | xlsx only. |
| `granularity` | `per_day`, `per_period` | `per_day` | See below. |
| `time_basis` | `registered`, `contract` | `registered` | Which minutes the `amount` field uses: worked or contract. |
| `rounding` | `none`, `nearest_five_minutes`, `nearest_quarter_hour`, `up_quarter_hour`, `down_quarter_hour` | `none` | Applied per day to worked and contract minutes. |
| `skip_zero` | bool | `false` | Drop rows whose `amount` is zero. |

**Granularity.** `per_day`: rows with the same person, date, kind (and site, only when a `site_code`
column exists) are merged: minutes are summed, start is the earliest, stop the latest. Upstream should
therefore give contract minutes on one row per day only. `per_period`: after the per-day merge and
rounding, minutes are summed per person and kind; `date`, `start`, `stop`, `site_code` and
`source_mode` are not available (use `period_from` / `period_to`).

### Code mapping

```toml
[codes]
worked = "100"        # required when a `code` column is used

[codes.absence]
VAC = "200"           # absence code -> output code
```

An absence row without a mapping fails the export (`MissingCode`); nothing is dropped silently.

### Columns

Ordered list; each column has a `header`, a `field` and an optional `format`.

```toml
[[columns]]
header = "datum"
field = "date"
format = "%d/%m/%Y"
```

| Field | Content | Format |
| --- | --- | --- |
| `person_key` | payroll / contract number | none |
| `person_name` | name | none |
| `site_code` | site (cost centre), empty if none | none |
| `date` | day | strftime, default `%Y-%m-%d` |
| `start`, `stop` | clock times, empty if missing | strftime, default `%H:%M` |
| `break`, `worked`, `contract` | durations | duration, default `hours_decimal` |
| `amount` | `worked` or `contract` per `time_basis` | duration |
| `code` | output code from `[codes]` | none |
| `source_mode` | `registered` or `contract` | none |
| `period_from`, `period_to` | export period | strftime |
| `organisation` | organisation name | none |

Duration formats: `minutes` (integer), `hours_decimal` (2 decimals), `hours_decimal(n)` with n 0..6
(half-up rounding), `hh:mm`. In XLSX, dates are real date cells (the strftime pattern is translated to an
Excel number format; `%Y %y %m %d %b %B` are supported, anything else falls back to `yyyy-mm-dd`), and
`minutes` / `hours_decimal` are numeric cells; `hh:mm` and times are text.

## Rows without a payroll number

If any row has an empty `person_key`, the export fails with `MissingPersonKey` listing the persons.
People are never silently dropped.

## Shipped profiles

- `generic-csv`: all fields, one row per person per day, `;`, decimal comma, UTF-8.
- `generic-xlsx`: the same as one typed Excel sheet.
- `acerta-kalender`: see below.

## Acerta

Acerta Connect imports through configurable import definitions; the exact layout is not public (see
`docs/export-acerta.md`). `acerta-kalender` is an **inferred** layout:
`overeenkomst;datum;looncode;uren`, one row per employee per day per looncode, decimal comma, ISO date,
UTF-8. The person key must hold the Acerta overeenkomstnummer. The looncode `100` is a placeholder
(marked TODO in the file).

Knobs to change once Acerta's import definition is known: `separator`, `encoding` (maybe
`windows-1252`) and `bom`, `header`, `decimal_separator`, the `date` format (`%d/%m/%Y` is the likely
alternative), column order and headers, `[codes]` for presence and absence looncodes, `rounding`,
`skip_zero`, and `format = "xlsx"` if Excel is preferred.

What to ask Acerta:

1. The technical specification of the calendar import and a sample import definition (text or Excel).
2. Whether a standard definition exists or one is set up per customer.
3. The looncodes for presence and each absence type used by this employer.
4. Whether begin/end times are accepted or only date + looncode + hours.
5. Access to the REST API for prestaties, if available.
