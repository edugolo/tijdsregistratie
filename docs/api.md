# Public REST API v1

A small JSON API under `/api/v1` for integrations (HR sync, payroll, reporting). It is separate from
the browser app: it uses API keys only, never cookies or browser sessions.

Reference: `/api/v1/docs` (interactive) and `/api/v1/openapi.json` (OpenAPI 3.1). Both are **off by
default**; switch them on with `API_DOCS=on`. The API itself works either way.

## Create a key

Only admins manage keys (server functions `api_keys`, `create_api_key`, `revoke_api_key`; the
Instellingen > Integraties page is built on them). HR and managers cannot see or change keys.

A key has a name, one or more scopes, an optional site restriction and an optional expiry. The key
(`tr_live_` followed by 43 characters) is shown **once** when it is created. Only its SHA-256 hash
is stored, plus a display prefix (`tr_live_` and 8 characters) to recognise it in the list. Keys can
be revoked at any time; revoking is immediate and final. Creating and revoking are written to the
audit log.

Keys give access to personal data. Store them like passwords, give each integration its own key
with the fewest scopes and sites it needs, and set an expiry.

## Authentication

```sh
curl -H "Authorization: Bearer $TIJD_API_KEY" https://tijd.example.org/api/v1/people
```

Only the `Authorization: Bearer` header counts. Cookies, browser sessions and keys in the URL are
ignored, and the browser cross-site (Origin) check does not apply to `/api/v1`.

## Scopes

| Scope | Gives |
|---|---|
| `people:read` | read people |
| `people:write` | create, update and deactivate employees by payroll number (HR sync) |
| `hours:read` | read hours and the validation status of periods |
| `export:read` | download payroll exports |

A key restricted to sites only sees data of those sites. Asking for another site gives `403`
(also for a site that does not exist, so nothing leaks about other sites). A key without the
scope an endpoint needs gets `403`.

`people:write` is powerful: another program holding the key can add, change and deactivate
employees (within the key's sites). It can never create or change managers, HR or admins. Give it
only to a key you trust, restrict it to sites, and set an expiry date.

Restricted keys see less than the people they look at. In `GET /people` only the roles and sites
within the key's sites are listed (organisation-wide roles are hidden). In `GET /hours` only days
and events recorded at the key's sites are returned, also for people who work at other sites too;
`validated` then looks at those sites. A correction that adds an event has no site, so it is only
included for people who work exclusively at the key's sites. Without the restriction everything
is returned.

Failed authentications (unknown, revoked or expired key) are audited as `api.auth_failed` with a
keyed hash of the client address, at most a few rows per client and period. Successful `PUT`
calls are audited in the same transaction as the change; reads and refused calls by the API layer
(`api.call`).

## Endpoints

### `GET /api/v1/people?site=<uuid>&active=<bool>`

Scope `people:read`. Optional filters: `site` (people with a role at that site) and `active`.

```sh
curl -H "Authorization: Bearer $TIJD_API_KEY" \
  "https://tijd.example.org/api/v1/people?active=true"
```

```json
[
  {
    "id": "8d0c0b4e-2f4b-4c0a-9a54-0a6c2b9a7f10",
    "name": "Anna Voorbeeld",
    "active": true,
    "roles": ["employee"],
    "sites": ["3f1d6a52-6c1e-4f6e-8a3f-0b8c5d2e9a11"],
    "device_linked": true
  }
]
```

`roles` lists `employee`, `manager`, `hr` and `admin`; `sites` the sites the person has a role at
(admin and HR have none). A key restricted to sites sees the employees of those sites, and of each
only the roles and sites within the key's sites; an unrestricted key sees everyone.

### `PUT /api/v1/people/{external_id}`

Scope `people:write`. HR sync: create or update an employee, identified by payroll number
(`external_id`, 1 to 20 characters from `A-Z a-z 0-9 . _ / -`, not starting with `=`, `+`, `-` or
`@`, compared case-insensitively; it is stored as the person's payroll number and is what
`person_key` holds in exports; anything else is `400`).

```sh
curl -X PUT -H "Authorization: Bearer $TIJD_API_KEY" -H "Content-Type: application/json" \
  -d '{"name":"Anna Voorbeeld","role":"employee","site_ids":["3f1d6a52-6c1e-4f6e-8a3f-0b8c5d2e9a11"],"active":true}' \
  https://tijd.example.org/api/v1/people/P-0001
```

Answers `201` with the person (same shape as `GET /people`) when the number was unknown, `200`
when the person existed. The body is the full desired state: `name`, `role` (always `employee`,
at every listed site; any other value is `400`), `site_ids` (at least one active site; employee
roles at other sites are removed) and `active` (`false` deactivates and ends sessions, `true`
reactivates). An identical repeat changes nothing.

Rules: the API only creates and changes employees. An existing person is only changed when they
have role assignments, all of them `employee` at sites the key may use. Managers, HR, admins,
people without any role (the API cannot "claim" them) and people who also work at a site the key
may not use are refused with `409 conflict`, always the same answer, so a restricted key learns
nothing about people outside its sites. This also holds for deactivating and reactivating: the
API only deactivates or reactivates the employees it may manage, including employees that staff
deactivated. A key restricted to sites may only name its own sites (`403` otherwise). Passkeys
and devices are never touched; a new person still enrolls in person. Changes are audited
(`person.created`, `person.updated`) with field names and the key id, no values.

### `GET /api/v1/hours?from=&to=&site=`

Scope `hours:read`. Per person per day for `from` to `to` (inclusive, `YYYY-MM-DD` local dates,
at most 62 days), optionally for one site. Names are not included: use `person_id` and
`payroll_number` (absent when the person has none).

```json
[
  {
    "person_id": "8d0c0b4e-2f4b-4c0a-9a54-0a6c2b9a7f10",
    "payroll_number": "P-0001",
    "days": [
      {
        "date": "2026-03-02",
        "start": "2026-03-02T07:00:00Z",
        "stop": "2026-03-02T15:30:00Z",
        "break_minutes": 30,
        "worked_minutes": 480,
        "contract_minutes": 456,
        "validated": true,
        "corrected": false
      }
    ]
  }
]
```

Hours are the effective values after corrections. `validated` says whether the day lies in a
validated period; unvalidated days are returned too (they may still change).

### `GET /api/v1/periods?from=&to=&site=`

Scope `hours:read`. Validation status per site (at most 366 days): the active validations that
touch the range, the `unvalidated` stretches inside it and `fully_validated`. Only validated days
can be exported.

```json
[
  {
    "site_id": "3f1d6a52-6c1e-4f6e-8a3f-0b8c5d2e9a11",
    "site_name": "School A",
    "cost_centre": "A-100",
    "validated": [{ "from": "2026-03-01", "to": "2026-03-07", "validated_at": "2026-03-08T09:00:00Z" }],
    "unvalidated": [{ "from": "2026-03-08", "to": "2026-03-10" }],
    "fully_validated": false
  }
]
```

### `GET /api/v1/exports/{profile}?from=&to=&site=&basis=`

Scope `export:read`. Downloads the payroll export as a file (`Content-Disposition: attachment`).
`profile` is a shipped profile id (`generic-csv`, `generic-xlsx`, `acerta-kalender`) or the uuid of
an active custom profile (see `docs/export-profiles.md`). `site` limits to one site (default: every
site with employees that the key may see); `basis` (`registered` or `contract`) overrides the
profile's time basis. At most 366 days and 200 000 rows.

Only **finished, validated days** are exported:

- Days from today on (organisation time zone) are never part of an export and never count as
  "not validated". A period that ends today or later is cut off at yesterday; the batch (and
  the file name) records the cut period. A period without any past day is `400`.
- A day is exported under the site where it was worked (the site of its first event, with that
  site's cost centre) and needs that site's validation, also when the site is not in the request.
  Days without events (contract days) fall under the person's first requested site.

The response carries `X-Export-Sha256` (checksum of the file) and `X-Export-Batch` (id of the
recorded batch). The same inputs give the same bytes and checksum, also compared with the
download in the app.

Nothing is produced (`409 conflict`) when a requested site is not validated for the whole period,
when anyone with hours has no payroll number, when a name, payroll number or cost centre holds a
character that the profile's file encoding (`windows-1252`) cannot represent, or when the profile
cannot be applied; check `GET /periods` first. The answer names nobody; the staff app lists the
persons so HR can fix the data.

CSV files are protected against spreadsheet formula injection: a text cell or header that starts
with `=`, `+`, `-`, `@`, a tab or a carriage return gets a leading `'`, so Excel and Calc show it as
text instead of running it. Plain numbers (`-5`, `-5,25`) and numeric columns are left alone. XLSX
files store all text as strings.

Generating an export is heavy, so it is limited: **10 exports per minute per key** and two exports
generated at a time on the whole server; beyond that the API answers `429`. More than 200 000 rows
(or a selection of more than 400 000 person-days) is `400`. The batch is recorded with the key (no creator) and audited as
`export.created`; HR and admins see it in the export history, marked as made through the API.
In the browser app, exports (preview, download, history, custom profiles) are limited to HR and
admins; managers have no export for now.

## Rate limit

120 requests per minute per key. Beyond that the API answers `429` with `Retry-After: 60`.
`GET /exports/{profile}` has its own, stricter limit of 10 per minute per key (see above).

## Errors

Every error is JSON, `{ "error": "<code>" }`:

| Status | Code | Meaning |
|---|---|---|
| 400 | `invalid_request` | invalid query or body |
| 401 | `unauthorized` | no key, unknown, revoked or expired key (`WWW-Authenticate: Bearer`) |
| 403 | `forbidden` | the key lacks the scope or may not see that site |
| 404 | `not_found` | unknown path or object |
| 409 | `conflict` | the data does not allow it (export of unvalidated days, without payroll numbers or with unencodable values) |
| 429 | `rate_limited` | rate limit exceeded, or the server is busy generating other exports |
| 500 | `internal` | server error, details only in the server log |

## Audit

Every call with a recognised key is written to the audit log as `api.call` with the key id, the
route pattern (never the raw URL, query string or body) and the response status. A call whose audit
row cannot be written is answered with `500` instead of its data. Calls without a valid key are
only logged as warnings.

## Content Security Policy and what the reference shows

`/api/v1/docs` sets its own Content Security Policy, stricter than the app's (the app needs
`'unsafe-inline'` for scripts because of Dioxus, the reference does not):

```
default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:;
font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none';
frame-ancestors 'none'
```

The Scalar script is vendored (`crates/app/vendor/scalar/`) and served from this origin as
`/api/v1/docs/scalar.js`; the page has no inline script (its configuration is a `data-` attribute).
Scalar only needs inline styles (it injects `<style>` elements) and `data:` images and fonts.

The Scalar configuration switches off everything that talks to its cloud or offers tools: no "Ask
AI" agent, no MCP, no developer tools (configure, share, deploy, SDK), no "Open API client" or
"Test request" button (nothing is sent from the page, so no key is typed into it), no telemetry, no
external fonts and no proxy. The end-to-end test checks that none of these appear and that the page
requests nothing from another origin.
