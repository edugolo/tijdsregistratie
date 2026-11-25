-- Short code that can be typed instead of scanning the QR code of an enrollment token. Only the
-- SHA-256 of the normalised code is stored, on the token's own row: the code shares the token's
-- person, expiry and single use. NULL for tokens issued without a code.
ALTER TABLE enrollment_token
    ADD COLUMN code_hash bytea UNIQUE CHECK (code_hash IS NULL OR octet_length(code_hash) = 32);
