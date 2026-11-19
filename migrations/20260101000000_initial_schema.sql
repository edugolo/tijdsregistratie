-- Version 1 schema. Personal data columns (*_enc) hold encrypted blobs:
-- key_version (1 byte) || nonce (24 bytes) || ciphertext+tag.

CREATE TABLE organisation_settings (
    id                 smallint PRIMARY KEY CHECK (id = 1),
    display_name       text NOT NULL DEFAULT '',
    brand_color        text,
    location_retention_days integer NOT NULL DEFAULT 31 CHECK (location_retention_days >= 0),
    max_accuracy_m     real NOT NULL DEFAULT 150,
    tolerance_m        real NOT NULL DEFAULT 50,
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE site (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name                text NOT NULL,
    address_enc         bytea,
    lat                 double precision,
    lng                 double precision,
    radius_m            real NOT NULL CHECK (radius_m > 0),
    break_minutes       integer NOT NULL DEFAULT 30,
    break_after_minutes integer NOT NULL DEFAULT 360,
    zone_policy         text NOT NULL DEFAULT 'flag' CHECK (zone_policy IN ('flag', 'block')),
    active              boolean NOT NULL DEFAULT true,
    created_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE person (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    webauthn_user_handle  bytea NOT NULL UNIQUE,
    display_name_enc      bytea NOT NULL,
    email_enc             bytea,
    name_index            bytea,
    active                boolean NOT NULL DEFAULT true,
    created_by            uuid REFERENCES person (id),
    created_at            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX person_name_index_idx ON person (name_index);

CREATE TABLE role_assignment (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    person_id   uuid NOT NULL REFERENCES person (id) ON DELETE CASCADE,
    role        text NOT NULL CHECK (role IN ('employee', 'manager', 'admin')),
    site_id     uuid REFERENCES site (id),
    created_by  uuid REFERENCES person (id),
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((role = 'admin') = (site_id IS NULL))
);
CREATE UNIQUE INDEX role_assignment_unique_idx
    ON role_assignment (person_id, role, COALESCE(site_id, '00000000-0000-0000-0000-000000000000'::uuid));
CREATE INDEX role_assignment_site_idx ON role_assignment (site_id);

CREATE TABLE enrollment_token (
    token_hash  bytea PRIMARY KEY,
    person_id   uuid NOT NULL REFERENCES person (id),
    issued_by   uuid REFERENCES person (id),
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE passkey (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    person_id      uuid NOT NULL REFERENCES person (id),
    credential_id  bytea NOT NULL UNIQUE,
    credential     jsonb NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_used_at   timestamptz,
    revoked_at     timestamptz,
    revoked_by     uuid REFERENCES person (id)
);
CREATE INDEX passkey_person_idx ON passkey (person_id);

CREATE TABLE time_event (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    person_id    uuid NOT NULL REFERENCES person (id),
    kind         text NOT NULL CHECK (kind IN ('start', 'stop')),
    server_ts    timestamptz NOT NULL,
    client_ts    timestamptz,
    site_id      uuid REFERENCES site (id),
    location_enc bytea,
    zone_status  text NOT NULL CHECK (zone_status IN ('in_zone', 'out_of_zone', 'uncertain', 'no_location')),
    source       text NOT NULL DEFAULT 'personal' CHECK (source IN ('personal', 'kiosk', 'correction')),
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX time_event_person_ts_idx ON time_event (person_id, server_ts);

-- Append-only: only the coordinate purge (location_enc -> NULL, nothing else changed) is allowed.
CREATE FUNCTION time_event_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.location_enc IS NULL
       AND (to_jsonb(NEW) - 'location_enc') = (to_jsonb(OLD) - 'location_enc') THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'time_event is append-only (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER time_event_append_only
    BEFORE UPDATE OR DELETE ON time_event
    FOR EACH ROW EXECUTE FUNCTION time_event_append_only();

CREATE TABLE audit_log (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id       uuid,
    action         text NOT NULL,
    entity         text NOT NULL,
    entity_id      uuid,
    changed_fields text[] NOT NULL DEFAULT '{}',
    at             timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_log_entity_idx ON audit_log (entity, entity_id);
