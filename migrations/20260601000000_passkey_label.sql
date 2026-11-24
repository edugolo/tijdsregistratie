-- Non-personal description of the device a passkey was registered on, like "iPhone – Safari".
-- Derived on the server from the User-Agent at registration; never holds names or identifiers.
ALTER TABLE passkey
    ADD COLUMN label text CHECK (label IS NULL OR char_length(label) <= 60);
