-- First-run setup: organisation branding fields and one-time setup codes.
ALTER TABLE organisation_settings
    ADD COLUMN short_name         text NOT NULL DEFAULT '',
    ADD COLUMN logo               bytea,
    ADD COLUMN logo_mime          text,
    ADD COLUMN setup_completed_at timestamptz;

-- Only the SHA-256 hash of a code is stored. Codes are valid for 24 hours and single use.
CREATE TABLE setup_code (
    code_hash   bytea PRIMARY KEY,
    created_at  timestamptz NOT NULL DEFAULT now(),
    used_at     timestamptz
);
