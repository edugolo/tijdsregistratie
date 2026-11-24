# Public REST API v1

A small JSON API under `/api/v1` for integrations (HR sync, payroll, reporting). It is separate from
the browser app: it uses API keys only, never cookies or browser sessions.

Reference: `/api/v1/docs` (interactive) and `/api/v1/openapi.json` (OpenAPI 3.1). Both can be
switched off with `API_DOCS=off`; the API itself keeps working.

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
| `people:write` | create and update people by payroll number (HR sync) |
| `hours:read` | read hours and the validation status of periods |
| `export:read` | download payroll exports |

A key restricted to sites only sees data of those sites. Asking for another site gives `403`.
A key without the scope an endpoint needs gets `403`.

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
(admin and HR have none). A key restricted to sites sees the employees of those sites; an
unrestricted key sees everyone.

### `PUT /api/v1/people/{external_id}`

Scope `people:write`. HR sync: create or update a person, identified by payroll number
(`external_id`, 1 to 20 characters, compared case-insensitively; it is stored as the person's
payroll number and is what `person_key` holds in exports).

```sh
curl -X PUT -H "Authorization: Bearer $TIJD_API_KEY" -H "Content-Type: application/json" \
  -d '{"name":"Anna Voorbeeld","role":"employee","site_ids":["3f1d6a52-6c1e-4f6e-8a3f-0b8c5d2e9a11"],"active":true}' \
  https://tijd.example.org/api/v1/people/P-0001
```

Answers `201` with the person (same shape as `GET /people`) when the number was unknown, `200`
when the person existed. The body is the full desired state: `name`, `role` (`employee` or
`manager`, at every listed site), `site_ids` (at least one active site; roles at other sites are
removed) and `active` (`false` deactivates and ends sessions). An identical repeat changes nothing.

Rules: HR and admin can never be created or changed through the API (`403` when the number belongs
to one; any other `role` value is `400`). A key restricted to sites may only name its own sites and
only change people who work exclusively at them (`403` otherwise). Passkeys and devices are never
touched; a new person still enrolls in person. Changes are audited (`person.created`,
`person.updated`, `role.changed`) with field names and the key id, no values.

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
profile's time basis. At most 366 days.

The response carries `X-Export-Sha256` (checksum of the file) and `X-Export-Batch` (id of the
recorded batch). The same inputs give the same bytes and checksum, also compared with the
download in the app.

Nothing is produced (`409 conflict`) when a requested site is not validated for the whole period,
when anyone with hours has no payroll number, or when the profile cannot be applied; check
`GET /periods` first. The batch is recorded with the key (no creator) and audited as
`export.created`; HR and admins see it in the export history, marked as made through the API.
In the browser app, exports (preview, download, history, custom profiles) are limited to HR and
admins; managers have no export for now.

## Rate limit

120 requests per minute per key. Beyond that the API answers `429` with `Retry-After: 60`.

## Errors

Every error is JSON, `{ "error": "<code>" }`:

| Status | Code | Meaning |
|---|---|---|
| 400 | `invalid_request` | invalid query or body |
| 401 | `unauthorized` | no key, unknown, revoked or expired key (`WWW-Authenticate: Bearer`) |
| 403 | `forbidden` | the key lacks the scope or may not see that site |
| 404 | `not_found` | unknown path or object |
| 409 | `conflict` | the data does not allow it (export of unvalidated days or without payroll numbers) |
| 429 | `rate_limited` | rate limit exceeded |
| 500 | `internal` | server error, details only in the server log |

## Audit

Every call with a recognised key is written to the audit log as `api.call` with the key id, the
route pattern (never the raw URL, query string or body) and the response status. A call whose audit
row cannot be written is answered with `500` instead of its data. Calls without a valid key are
only logged as warnings.

## Content Security Policy

`/api/v1/docs` runs under the app's regular CSP. The Scalar script is vendored
(`crates/app/vendor/scalar/`) and served from this origin as `/api/v1/docs/scalar.js`, and the page has
no inline script, so no relaxed policy is needed.
