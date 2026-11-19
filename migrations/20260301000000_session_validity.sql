-- Sessions authenticated before this instant are rejected. Bumped when a person's passkeys are
-- revoked or the person is deactivated, so signed-in devices lose access immediately.
ALTER TABLE person
    ADD COLUMN sessions_valid_after timestamptz NOT NULL DEFAULT '-infinity';
