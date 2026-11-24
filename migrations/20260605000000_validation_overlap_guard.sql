-- Defence in depth: the database itself refuses a new active validation that overlaps an active
-- validation of the same site. The repository checks this too, under the same per-site advisory
-- lock; taking that lock here makes the check race-free for any writer.
CREATE FUNCTION period_validation_no_overlap() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.reopened_at IS NOT NULL THEN
        RETURN NEW;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('period_validation:' || NEW.site_id::text, 0));
    IF EXISTS (
        SELECT 1 FROM period_validation
        WHERE site_id = NEW.site_id
          AND reopened_at IS NULL
          AND period_start <= NEW.period_end
          AND period_end >= NEW.period_start
    ) THEN
        RAISE EXCEPTION 'overlapping active period_validation for site %', NEW.site_id
            USING ERRCODE = '23P01';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER period_validation_no_overlap
    BEFORE INSERT ON period_validation
    FOR EACH ROW EXECUTE FUNCTION period_validation_no_overlap();
