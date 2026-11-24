-- HR (personeelsdienst): an organisation-wide role like admin, so it has no site. Employee and
-- manager keep requiring a site. The unique index (person, role, site-or-nil) is unchanged.
DO $$
DECLARE
    c record;
BEGIN
    FOR c IN SELECT conname FROM pg_constraint
             WHERE conrelid = 'role_assignment'::regclass AND contype = 'c'
    LOOP
        EXECUTE format('ALTER TABLE role_assignment DROP CONSTRAINT %I', c.conname);
    END LOOP;
END $$;

ALTER TABLE role_assignment
    ADD CONSTRAINT role_assignment_role_check
        CHECK (role IN ('employee', 'manager', 'admin', 'hr')),
    ADD CONSTRAINT role_assignment_site_scope_check
        CHECK ((role IN ('admin', 'hr')) = (site_id IS NULL));
