-- API keys for the public REST API (/api/v1). Only the SHA-256 hash of a key is stored; the key
-- itself is shown once at creation. The name is a label chosen by an admin, not personal data.
CREATE TABLE api_key (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    -- Non-secret display prefix: the first characters after `tr_live_`.
    prefix       text NOT NULL,
    key_hash     bytea NOT NULL UNIQUE,
    scopes       text[] NOT NULL
        CHECK (cardinality(scopes) > 0
               AND scopes <@ ARRAY['people:read', 'people:write', 'hours:read', 'export:read']),
    -- NULL: all sites.
    site_ids     uuid[],
    created_by   uuid REFERENCES person (id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz,
    last_used_at timestamptz,
    revoked_at   timestamptz,
    revoked_by   uuid REFERENCES person (id)
);
