#![cfg(feature = "server")]

use sqlx::PgPool;
use uuid::Uuid;

async fn person(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO person (id, webauthn_user_handle, display_name_enc) VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(id.as_bytes().to_vec())
    .bind(vec![1u8, 2, 3])
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn site(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO site (id, name, radius_m) VALUES ($1, 'School', 100)")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn event(pool: &PgPool, person: Uuid, kind: &str, zone: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO time_event (id, person_id, kind, server_ts, zone_status, location_enc) \
         VALUES ($1, $2, $3, now(), $4, $5)",
    )
    .bind(id)
    .bind(person)
    .bind(kind)
    .bind(zone)
    .bind(vec![9u8])
    .execute(pool)
    .await?;
    Ok(id)
}

async fn role(pool: &PgPool, p: Uuid, role: &str, site: Option<Uuid>) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO role_assignment (person_id, role, site_id) VALUES ($1, $2, $3)")
        .bind(p)
        .bind(role)
        .bind(site)
        .execute(pool)
        .await
        .map(|_| ())
}

#[sqlx::test(migrations = "../../migrations")]
async fn migrations_apply(pool: PgPool) {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' \
         AND table_name IN ('organisation_settings','site','person','role_assignment', \
         'enrollment_token','passkey','time_event','audit_log')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 8);
}

#[sqlx::test(migrations = "../../migrations")]
async fn enum_checks_reject_bad_values(pool: PgPool) {
    let p = person(&pool).await;
    let s = site(&pool).await;
    assert!(event(&pool, p, "pause", "in_zone").await.is_err());
    assert!(event(&pool, p, "start", "nearby").await.is_err());
    assert!(event(&pool, p, "start", "in_zone").await.is_ok());
    assert!(role(&pool, p, "janitor", Some(s)).await.is_err());
    assert!(
        sqlx::query("INSERT INTO site (id, name, radius_m, zone_policy) VALUES ($1,'x',10,'nope')")
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO site (id, name, radius_m) VALUES ($1,'x',0)")
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "INSERT INTO time_event (id, person_id, kind, server_ts, zone_status, source) \
             VALUES ($1,$2,'start',now(),'in_zone','bogus')"
        )
        .bind(Uuid::new_v4())
        .bind(p)
        .execute(&pool)
        .await
        .is_err()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_requires_null_site_others_require_site(pool: PgPool) {
    let p = person(&pool).await;
    let s = site(&pool).await;
    assert!(role(&pool, p, "admin", Some(s)).await.is_err());
    assert!(role(&pool, p, "manager", None).await.is_err());
    assert!(role(&pool, p, "employee", None).await.is_err());
    assert!(role(&pool, p, "admin", None).await.is_ok());
    assert!(role(&pool, p, "manager", Some(s)).await.is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn hr_requires_null_site_like_admin(pool: PgPool) {
    let p = person(&pool).await;
    let s = site(&pool).await;
    assert!(role(&pool, p, "hr", Some(s)).await.is_err());
    assert!(role(&pool, p, "hr", None).await.is_ok());
    // The unique index treats the missing site as one value: a second hr row is a duplicate.
    assert!(role(&pool, p, "hr", None).await.is_err());
    // Other roles keep their rules.
    assert!(role(&pool, p, "manager", None).await.is_err());
    assert!(role(&pool, p, "employee", Some(s)).await.is_ok());
    assert!(role(&pool, p, "admin", Some(s)).await.is_err());
    assert!(role(&pool, p, "janitor", None).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_roles_rejected(pool: PgPool) {
    let p = person(&pool).await;
    let s = site(&pool).await;
    role(&pool, p, "employee", Some(s)).await.unwrap();
    assert!(role(&pool, p, "employee", Some(s)).await.is_err());
    role(&pool, p, "admin", None).await.unwrap();
    assert!(role(&pool, p, "admin", None).await.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn time_event_is_append_only(pool: PgPool) {
    let p = person(&pool).await;
    let id = event(&pool, p, "start", "in_zone").await.unwrap();

    let upd = sqlx::query("UPDATE time_event SET kind = 'stop' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(upd.is_err());
    let upd = sqlx::query("UPDATE time_event SET server_ts = now() WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(upd.is_err());
    let del = sqlx::query("DELETE FROM time_event WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(del.is_err());

    sqlx::query("UPDATE time_event SET location_enc = NULL WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let loc: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT location_enc FROM time_event WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(loc.is_none());
}
