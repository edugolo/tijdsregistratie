//! Stored WebAuthn passkeys. The serialized credential is public-key material, not personal data.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

/// A stored passkey row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredPasskey {
    pub id: Uuid,
    pub person_id: Uuid,
    pub credential: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Stores a new passkey. `credential_json` is the serialized public credential (no personal data).
pub async fn insert(
    pool: &PgPool,
    person_id: Uuid,
    credential_id: &[u8],
    credential_json: serde_json::Value,
) -> Result<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO passkey (person_id, credential_id, credential) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(person_id)
    .bind(credential_id)
    .bind(credential_json)
    .fetch_one(pool)
    .await?)
}

/// Non-revoked passkeys of a person as `(row id, credential json)`, oldest first.
pub async fn active_for_person(
    pool: &PgPool,
    person_id: Uuid,
) -> Result<Vec<(Uuid, serde_json::Value)>> {
    Ok(sqlx::query_as(
        "SELECT id, credential FROM passkey \
         WHERE person_id = $1 AND revoked_at IS NULL ORDER BY created_at, id",
    )
    .bind(person_id)
    .fetch_all(pool)
    .await?)
}

/// Looks a passkey up by its WebAuthn credential id, revoked ones included.
pub async fn find_by_credential_id(
    pool: &PgPool,
    credential_id: &[u8],
) -> Result<Option<StoredPasskey>> {
    Ok(sqlx::query_as(
        "SELECT id, person_id, credential, created_at, last_used_at, revoked_at \
         FROM passkey WHERE credential_id = $1",
    )
    .bind(credential_id)
    .fetch_optional(pool)
    .await?)
}

/// Stores the updated credential state (counter, backup flags) and sets `last_used_at`.
/// Returns false if the passkey does not exist.
pub async fn update_after_login(
    pool: &PgPool,
    id: Uuid,
    credential_json: serde_json::Value,
) -> Result<bool> {
    let done =
        sqlx::query("UPDATE passkey SET credential = $2, last_used_at = now() WHERE id = $1")
            .bind(id)
            .bind(credential_json)
            .execute(pool)
            .await?;
    Ok(done.rows_affected() > 0)
}

/// Number of non-revoked passkeys of a person.
pub async fn count_active(pool: &PgPool, person_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM passkey WHERE person_id = $1 AND revoked_at IS NULL",
    )
    .bind(person_id)
    .fetch_one(pool)
    .await?)
}

/// Whether the person registered a (non-revoked) passkey at or after `since`.
pub async fn has_active_since(
    pool: &PgPool,
    person_id: Uuid,
    since: DateTime<Utc>,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM passkey \
         WHERE person_id = $1 AND revoked_at IS NULL AND created_at >= $2)",
    )
    .bind(person_id)
    .bind(since)
    .fetch_one(pool)
    .await?)
}

/// Revokes all active passkeys of a person. Returns how many were revoked.
pub async fn revoke_all(pool: &PgPool, person_id: Uuid, revoked_by: Option<Uuid>) -> Result<u64> {
    let done = sqlx::query(
        "UPDATE passkey SET revoked_at = now(), revoked_by = $2 \
         WHERE person_id = $1 AND revoked_at IS NULL",
    )
    .bind(person_id)
    .bind(revoked_by)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};
    use serde_json::json;

    async fn person(pool: &PgPool) -> Uuid {
        let keys = DataKeys::parse(&generate_key_line(1)).unwrap();
        let blind = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        super::super::person::create_person(pool, &keys, &blind, "Test Persoon", None)
            .await
            .unwrap()
            .id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn insert_and_lookup(pool: PgPool) {
        let p = person(&pool).await;
        let id = insert(&pool, p, b"cred-1", json!({"counter": 0}))
            .await
            .unwrap();
        assert_eq!(count_active(&pool, p).await.unwrap(), 1);
        let active = active_for_person(&pool, p).await.unwrap();
        assert_eq!(active, vec![(id, json!({"counter": 0}))]);

        let found = find_by_credential_id(&pool, b"cred-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.person_id, p);
        assert!(found.revoked_at.is_none());
        assert!(
            find_by_credential_id(&pool, b"other")
                .await
                .unwrap()
                .is_none()
        );
        // Credential ids are unique across all people.
        assert!(insert(&pool, p, b"cred-1", json!({})).await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_after_login_sets_state_and_last_used(pool: PgPool) {
        let p = person(&pool).await;
        let id = insert(&pool, p, b"cred-1", json!({"counter": 0}))
            .await
            .unwrap();
        assert!(
            update_after_login(&pool, id, json!({"counter": 5}))
                .await
                .unwrap()
        );
        let found = find_by_credential_id(&pool, b"cred-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.credential, json!({"counter": 5}));
        assert!(found.last_used_at.is_some());
        assert!(
            !update_after_login(&pool, Uuid::new_v4(), json!({}))
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoke_all_hides_passkeys_and_is_scoped(pool: PgPool) {
        let (a, b) = (person(&pool).await, person(&pool).await);
        insert(&pool, a, b"a1", json!({})).await.unwrap();
        insert(&pool, a, b"a2", json!({})).await.unwrap();
        insert(&pool, b, b"b1", json!({})).await.unwrap();
        assert_eq!(revoke_all(&pool, a, Some(b)).await.unwrap(), 2);
        assert_eq!(revoke_all(&pool, a, Some(b)).await.unwrap(), 0);
        assert_eq!(count_active(&pool, a).await.unwrap(), 0);
        assert!(active_for_person(&pool, a).await.unwrap().is_empty());
        assert_eq!(count_active(&pool, b).await.unwrap(), 1);
        let revoked = find_by_credential_id(&pool, b"a1").await.unwrap().unwrap();
        assert!(revoked.revoked_at.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn has_active_since_ignores_old_and_revoked(pool: PgPool) {
        let p = person(&pool).await;
        let t0 = Utc::now() - chrono::Duration::seconds(5);
        assert!(!has_active_since(&pool, p, t0).await.unwrap());
        insert(&pool, p, b"c", json!({})).await.unwrap();
        assert!(has_active_since(&pool, p, t0).await.unwrap());
        assert!(
            !has_active_since(&pool, p, Utc::now() + chrono::Duration::seconds(60))
                .await
                .unwrap()
        );
        revoke_all(&pool, p, None).await.unwrap();
        assert!(!has_active_since(&pool, p, t0).await.unwrap());
    }
}
