-- Hours: payroll number, cost centre, contract minutes, corrections and period validation.

-- Payroll number: personal data, so encrypted; the blind index serves exact lookups and keeps
-- the number unique.
ALTER TABLE person
    ADD COLUMN payroll_number_enc   bytea,
    ADD COLUMN payroll_number_index bytea;
CREATE UNIQUE INDEX person_payroll_number_index_idx
    ON person (payroll_number_index) WHERE payroll_number_index IS NOT NULL;

-- Cost centre of a site: organisation configuration, not personal data.
ALTER TABLE site ADD COLUMN cost_centre text;

-- Contract minutes per ISO weekday (1 = Monday .. 7 = Sunday).
CREATE TABLE work_schedule (
    person_id  uuid NOT NULL REFERENCES person (id),
    weekday    smallint NOT NULL CHECK (weekday BETWEEN 1 AND 7),
    minutes    integer NOT NULL CHECK (minutes BETWEEN 0 AND 1440),
    PRIMARY KEY (person_id, weekday)
);

-- Corrections never touch time_event. A correction targets
--   * a time_event (time_event_id): replaces its time, or voids it,
--   * an earlier addition (target_correction_id): same, for an event that was added by hand,
--   * nothing: adds a missing event.
-- The latest correction per target wins. The reason is encrypted.
CREATE TABLE correction (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    time_event_id         uuid REFERENCES time_event (id),
    target_correction_id  uuid REFERENCES correction (id),
    person_id             uuid NOT NULL REFERENCES person (id),
    kind                  text NOT NULL CHECK (kind IN ('start', 'stop')),
    corrected_ts          timestamptz NOT NULL,
    reason_enc            bytea NOT NULL,
    created_by            uuid NOT NULL REFERENCES person (id),
    created_at            timestamptz NOT NULL DEFAULT clock_timestamp(),
    voids_event           boolean NOT NULL DEFAULT false,
    CHECK (time_event_id IS NULL OR target_correction_id IS NULL),
    CHECK (NOT voids_event OR time_event_id IS NOT NULL OR target_correction_id IS NOT NULL)
);
CREATE INDEX correction_person_ts_idx ON correction (person_id, corrected_ts);
CREATE INDEX correction_event_idx ON correction (time_event_id) WHERE time_event_id IS NOT NULL;
CREATE INDEX correction_target_idx ON correction (target_correction_id)
    WHERE target_correction_id IS NOT NULL;

CREATE FUNCTION correction_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'correction is append-only (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER correction_append_only
    BEFORE UPDATE OR DELETE ON correction
    FOR EACH ROW EXECUTE FUNCTION correction_append_only();

CREATE TRIGGER correction_no_truncate
    BEFORE TRUNCATE ON correction
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();

-- A validated (approved) period of a site, both ends inclusive. Reopening only fills in
-- reopened_by/reopened_at; the row stays. Overlaps of active validations of a site are refused by
-- the repository under a per-site lock.
CREATE TABLE period_validation (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id       uuid NOT NULL REFERENCES site (id),
    period_start  date NOT NULL,
    period_end    date NOT NULL,
    validated_by  uuid NOT NULL REFERENCES person (id),
    validated_at  timestamptz NOT NULL DEFAULT now(),
    reopened_by   uuid REFERENCES person (id),
    reopened_at   timestamptz,
    CHECK (period_end >= period_start),
    CHECK ((reopened_by IS NULL) = (reopened_at IS NULL))
);
CREATE INDEX period_validation_site_idx ON period_validation (site_id, period_start, period_end);

CREATE FUNCTION period_validation_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE'
       AND OLD.reopened_at IS NULL
       AND NEW.reopened_at IS NOT NULL
       AND (to_jsonb(NEW) - 'reopened_by' - 'reopened_at')
           = (to_jsonb(OLD) - 'reopened_by' - 'reopened_at') THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'period_validation only allows reopening (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER period_validation_guard
    BEFORE UPDATE OR DELETE ON period_validation
    FOR EACH ROW EXECUTE FUNCTION period_validation_guard();

CREATE TRIGGER period_validation_no_truncate
    BEFORE TRUNCATE ON period_validation
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();
