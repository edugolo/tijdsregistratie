//! First-admin bootstrap: person, admin role, audit rows and a 30-minute enrollment token.

use anyhow::{Result, bail};
use chrono::Duration;
use crypto::{BlindIndexKey, DataKeys};
use qrcode::QrCode;
use qrcode::render::unicode;
use sqlx::PgPool;
use store::repo::role::{self, Role};
use store::repo::{audit, enrollment, person};
use uuid::Uuid;

const TOKEN_TTL_MINUTES: i64 = 30;

/// Pure refusal rule: an active admin exists and `--force` was not given.
pub fn check_can_bootstrap(active_admins: i64, force: bool) -> Result<()> {
    if active_admins > 0 && !force {
        bail!(
            "an active admin already exists ({active_admins}); use --force to create another one"
        );
    }
    Ok(())
}

pub async fn count_active_admins(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(DISTINCT p.id) FROM person p \
         JOIN role_assignment ra ON ra.person_id = p.id \
         WHERE ra.role = 'admin' AND p.active",
    )
    .fetch_one(pool)
    .await?)
}

/// `{origin}/koppel#{raw}` (the code is in the fragment: never sent to servers); a trailing slash on the origin is ignored.
pub fn enrollment_url(origin: &str, raw: &str) -> String {
    format!("{}/koppel#{raw}", origin.trim_end_matches('/'))
}

pub fn render_qr(url: &str) -> Result<String> {
    let code = QrCode::new(url.as_bytes())?;
    Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

/// Creates the admin and returns the enrollment URL.
pub async fn bootstrap(
    pool: &PgPool,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    origin: &str,
    name: &str,
    force: bool,
) -> Result<(Uuid, String)> {
    check_can_bootstrap(count_active_admins(pool).await?, force)?;
    let p = person::create_person(pool, keys, blind, name, None).await?;
    audit::record(pool, None, "person.created", "person", Some(p.id), &[]).await?;
    role::assign(pool, p.id, Role::Admin, None, None).await?;
    audit::record(pool, None, "role.assigned", "person", Some(p.id), &["role"]).await?;
    let token =
        enrollment::issue_token(pool, p.id, None, Duration::minutes(TOKEN_TTL_MINUTES)).await?;
    Ok((p.id, enrollment_url(origin, &token.raw)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::generate_key_line;

    #[test]
    fn refuses_when_admin_exists_unless_forced() {
        assert!(check_can_bootstrap(0, false).is_ok());
        assert!(check_can_bootstrap(1, false).is_err());
        assert!(check_can_bootstrap(1, true).is_ok());
    }

    #[test]
    fn url_has_no_double_slash() {
        assert_eq!(
            enrollment_url("https://tijd.example.org/", "abc"),
            "https://tijd.example.org/koppel#abc"
        );
        assert_eq!(
            enrollment_url("https://tijd.example.org", "abc"),
            "https://tijd.example.org/koppel#abc"
        );
    }

    #[test]
    fn qr_renders_multiline_block_art() {
        let qr = render_qr("https://tijd.example.org/koppel#abc").unwrap();
        assert!(qr.lines().count() > 10);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn counts_only_active_admins(pool: PgPool) {
        let k = DataKeys::parse(&generate_key_line(1)).unwrap();
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        assert_eq!(count_active_admins(&pool).await.unwrap(), 0);
        let p = person::create_person(&pool, &k, &b, "Test Admin", None)
            .await
            .unwrap();
        role::assign(&pool, p.id, Role::Admin, None, None)
            .await
            .unwrap();
        assert_eq!(count_active_admins(&pool).await.unwrap(), 1);
        person::set_active(&pool, p.id, false).await.unwrap();
        assert_eq!(count_active_admins(&pool).await.unwrap(), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn bootstrap_creates_admin_and_url_then_refuses(pool: PgPool) {
        let k = DataKeys::parse(&generate_key_line(1)).unwrap();
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        let (id, url) = bootstrap(
            &pool,
            &k,
            &b,
            "https://tijd.example.org",
            "Test Admin",
            false,
        )
        .await
        .unwrap();
        assert!(url.starts_with("https://tijd.example.org/koppel#"));
        assert!(role::is_admin(&pool, id).await.unwrap());
        let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE entity_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audits, 2);
        assert!(
            bootstrap(&pool, &k, &b, "https://tijd.example.org", "Other", false)
                .await
                .is_err()
        );
    }
}
