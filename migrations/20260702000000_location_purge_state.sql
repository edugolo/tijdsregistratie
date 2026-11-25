-- Last run of the automatic retention purge (coordinates and clock notes), shown to admins on
-- the settings screen.
ALTER TABLE organisation_settings
    ADD COLUMN last_location_purge_at timestamptz,
    ADD COLUMN last_location_purge_count bigint CHECK (last_location_purge_count >= 0),
    ADD COLUMN last_note_purge_count bigint CHECK (last_note_purge_count >= 0);
