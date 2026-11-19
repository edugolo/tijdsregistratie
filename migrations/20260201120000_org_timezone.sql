-- Time zone in which "today" is determined for day totals and the start/stop sequence.
ALTER TABLE organisation_settings
    ADD COLUMN timezone text NOT NULL DEFAULT 'Europe/Brussels';
