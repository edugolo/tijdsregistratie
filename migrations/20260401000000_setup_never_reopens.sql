-- Setup never re-opens by itself: `organisation_settings.setup_completed_at` is set at the first
-- passkey of an admin (by the database, so no code path can forget it) and can never be cleared.
-- Recovery after losing every device is the operator CLI only (`admin bootstrap-admin --force`).
-- Written to be re-runnable.

-- The settings row always exists, so reads never have to insert it.
INSERT INTO organisation_settings (id) VALUES (1) ON CONFLICT DO NOTHING;

CREATE OR REPLACE FUNCTION mark_setup_completed() RETURNS void
LANGUAGE sql AS $$
    UPDATE organisation_settings SET setup_completed_at = now()
    WHERE id = 1 AND setup_completed_at IS NULL;
$$;

-- A passkey registered for a person who is an admin.
CREATE OR REPLACE FUNCTION passkey_mark_setup_completed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM role_assignment ra
               WHERE ra.person_id = NEW.person_id AND ra.role = 'admin') THEN
        PERFORM mark_setup_completed();
    END IF;
    RETURN NULL;
END;
$$;
DROP TRIGGER IF EXISTS passkey_marks_setup_completed ON passkey;
CREATE TRIGGER passkey_marks_setup_completed
    AFTER INSERT ON passkey
    FOR EACH ROW EXECUTE FUNCTION passkey_mark_setup_completed();

-- The admin role granted to a person who already has a passkey.
CREATE OR REPLACE FUNCTION admin_role_mark_setup_completed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.role = 'admin'
       AND EXISTS (SELECT 1 FROM passkey pk WHERE pk.person_id = NEW.person_id) THEN
        PERFORM mark_setup_completed();
    END IF;
    RETURN NULL;
END;
$$;
DROP TRIGGER IF EXISTS admin_role_marks_setup_completed ON role_assignment;
CREATE TRIGGER admin_role_marks_setup_completed
    AFTER INSERT ON role_assignment
    FOR EACH ROW EXECUTE FUNCTION admin_role_mark_setup_completed();

CREATE OR REPLACE FUNCTION keep_setup_completed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.setup_completed_at IS NOT NULL THEN
        NEW.setup_completed_at := OLD.setup_completed_at;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS organisation_settings_keep_setup_completed ON organisation_settings;
CREATE TRIGGER organisation_settings_keep_setup_completed
    BEFORE UPDATE ON organisation_settings
    FOR EACH ROW
    EXECUTE FUNCTION keep_setup_completed();

-- Installations where an admin has (ever) had a passkey are completed already.
UPDATE organisation_settings SET setup_completed_at = now()
WHERE setup_completed_at IS NULL
  AND EXISTS (SELECT 1 FROM passkey pk
              JOIN role_assignment ra ON ra.person_id = pk.person_id AND ra.role = 'admin');
