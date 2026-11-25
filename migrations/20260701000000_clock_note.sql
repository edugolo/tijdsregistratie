-- Clock note: one short, optional explanation by the employee on their own start or stop that
-- was not in the zone. Free text, so encrypted (associated data clock_note || note || id).
-- Insert-only: no edits, no deletes; the only allowed update is the retention purge, which sets
-- note_enc to NULL and leaves every other column unchanged.

-- Lets the note reference the event together with its person, so a note can only belong to the
-- person of the event.
ALTER TABLE time_event ADD CONSTRAINT time_event_id_person_key UNIQUE (id, person_id);

CREATE TABLE clock_note (
    id          uuid PRIMARY KEY,
    event_id    uuid NOT NULL UNIQUE REFERENCES time_event (id),
    person_id   uuid NOT NULL REFERENCES person (id),
    note_enc    bytea,
    created_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (event_id, person_id) REFERENCES time_event (id, person_id)
);

CREATE FUNCTION clock_note_insert_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND NEW.note_enc IS NULL
       AND (to_jsonb(NEW) - 'note_enc') = (to_jsonb(OLD) - 'note_enc') THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'clock_note is insert-only (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER clock_note_insert_only
    BEFORE UPDATE OR DELETE ON clock_note
    FOR EACH ROW EXECUTE FUNCTION clock_note_insert_only();

CREATE TRIGGER clock_note_no_truncate
    BEFORE TRUNCATE ON clock_note
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();
