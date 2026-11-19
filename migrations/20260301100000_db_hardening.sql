-- Database hardening: no TRUNCATE on the append-only tables, and an append-only audit log.
-- Triggers are a safety net against bugs and a compromised app role; the table owner can still
-- disable them (see docs/operations.md for the least-privilege role setup).

CREATE FUNCTION forbid_truncate() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only (TRUNCATE not allowed)', TG_TABLE_NAME;
END;
$$;

CREATE TRIGGER time_event_no_truncate
    BEFORE TRUNCATE ON time_event
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();

CREATE TRIGGER audit_log_no_truncate
    BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_truncate();

CREATE FUNCTION audit_log_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only (% not allowed)', TG_OP;
END;
$$;

CREATE TRIGGER audit_log_append_only
    BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION audit_log_append_only();
