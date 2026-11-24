-- Payroll export: custom export profiles and the record of produced exports.
-- Neither table holds personal data: a profile is organisation configuration (TOML) and a batch
-- holds ids, a period, a row count and a checksum. The exported file itself is never stored; it
-- is regenerated deterministically from the same inputs.

CREATE TABLE export_profile (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    toml       text NOT NULL CHECK (length(toml) BETWEEN 1 AND 65536),
    created_by uuid REFERENCES person (id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    active     boolean NOT NULL DEFAULT true
);

CREATE TABLE export_batch (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Id of a shipped profile (`generic-csv`) or the uuid of a custom profile, as text.
    profile        text NOT NULL CHECK (length(profile) BETWEEN 1 AND 100),
    -- SHA-256 of the profile TOML that was used (a custom profile can be edited later).
    profile_sha256 text NOT NULL CHECK (length(profile_sha256) = 64),
    period_from    date NOT NULL,
    period_to      date NOT NULL CHECK (period_to >= period_from),
    site_ids       uuid[] NOT NULL CHECK (cardinality(site_ids) > 0),
    time_basis     text NOT NULL CHECK (time_basis IN ('registered', 'contract')),
    row_count      integer NOT NULL CHECK (row_count >= 0),
    sha256         text NOT NULL CHECK (length(sha256) = 64),
    -- NULL when an API key made the export.
    created_by     uuid REFERENCES person (id),
    api_key_id     uuid REFERENCES api_key (id),
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX export_batch_created_at_idx ON export_batch (created_at DESC);

CREATE FUNCTION export_batch_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'export_batch is append-only (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER export_batch_append_only
    BEFORE UPDATE OR DELETE ON export_batch
    FOR EACH ROW EXECUTE FUNCTION export_batch_append_only();

CREATE TRIGGER export_batch_no_truncate
    BEFORE TRUNCATE ON export_batch
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();
