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
| `people:write` | create and update people (HR sync, later) |
| `hours:read` | read hours and periods (later) |
| `export:read` | download payroll exports (later) |

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

More endpoints (hours, exports, periods, `PUT /people/{external_id}`) follow; the OpenAPI document
always lists what exists.

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
| 429 | `rate_limited` | rate limit exceeded |
| 500 | `internal` | server error, details only in the server log |

## Audit

Every call with a recognised key is written to the audit log as `api.call` with the key id, the
route pattern (never the raw URL, query string or body) and the response status. A call whose audit
row cannot be written is answered with `500` instead of its data. Calls without a valid key are
only logged as warnings.

## Content Security Policy

`/api/v1/docs` runs under the app's regular CSP. The Scalar script is vendored
(`public/vendor/scalar/`) and served from this origin as `/api/v1/docs/scalar.js`, and the page has
no inline script, so no relaxed policy is needed.
