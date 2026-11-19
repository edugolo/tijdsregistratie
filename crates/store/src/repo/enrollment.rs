//! One-time enrollment tokens. Raw tokens are never stored; only their SHA-256 hash is.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rand::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::{RepoError, Result};

/// A freshly issued token. `raw` is shown once (QR code / link) and never stored.
#[derive(Clone)]
pub struct IssuedToken {
    /// URL-safe base64 (no padding) of 32 random bytes.
    pub raw: String,
    pub person_id: Uuid,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("person_id", &self.person_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

fn hash(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// Issues a single-use token for `person_id`, valid for `ttl`. Earlier unused tokens of the
/// same person are invalidated.
pub async fn issue_token(
    pool: &PgPool,
    person_id: Uuid,
    issued_by: Option<Uuid>,
    ttl: Duration,
) -> Result<IssuedToken> {
    let mut bytes = [0u8; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| RepoError::InvalidValue("random source"))?;
    let raw = URL_SAFE_NO_PAD.encode(bytes);
    let issued_at = Utc::now();
    let expires_at = issued_at + ttl;

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE enrollment_token SET expires_at = now()          WHERE person_id = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(person_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO enrollment_token (token_hash, person_id, issued_by, expires_at, created_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(hash(&raw))
    .bind(person_id)
    .bind(issued_by)
    .bind(expires_at)
    .bind(issued_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(IssuedToken {
        raw,
        person_id,
        issued_at,
        expires_at,
    })
}

/// Returns the person the token belongs to if it is unused and not expired, without consuming it.
pub async fn peek_valid(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT person_id FROM enrollment_token \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(hash(raw))
    .fetch_optional(pool)
    .await?)
}

/// Atomically marks the token used and returns its person, or `None` if it is unknown, used or expired.
pub async fn consume(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "UPDATE enrollment_token SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() RETURNING person_id",
    )
    .bind(hash(raw))
    .fetch_optional(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{BlindIndexKey, DataKeys, generate_key_line};

    async fn person(pool: &PgPool) -> Uuid {
        let keys = DataKeys::parse(&generate_key_line(1)).unwrap();
        let blind = BlindIndexKey::parse(&generate_key_line(1)).unwrap();
        super::super::person::create_person(pool, &keys, &blind, "Test Persoon", None)
            .await
            .unwrap()
            .id
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn issue_peek_consume_once(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        assert_eq!(t.person_id, p);
        assert_eq!(t.raw.len(), 43, "32 bytes, base64url without padding");
        assert!(!t.raw.contains(['=', '+', '/']));
        assert!(t.expires_at > t.issued_at);

        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), Some(p));
        // Peeking does not consume.
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), Some(p));
        assert_eq!(consume(&pool, &t.raw).await.unwrap(), Some(p));
        assert_eq!(consume(&pool, &t.raw).await.unwrap(), None);
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn raw_token_is_not_stored(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        let hash: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM enrollment_token")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hash.len(), 32);
        assert_ne!(hash, t.raw.as_bytes());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn expired_and_unknown_tokens_are_rejected(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::seconds(-5))
            .await
            .unwrap();
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), None);
        assert_eq!(consume(&pool, &t.raw).await.unwrap(), None);
        assert_eq!(peek_valid(&pool, "nope").await.unwrap(), None);
        assert_eq!(consume(&pool, "nope").await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn issuing_invalidates_earlier_tokens_of_the_same_person_only(pool: PgPool) {
        let (a, b) = (person(&pool).await, person(&pool).await);
        let old = issue_token(&pool, a, None, Duration::minutes(15))
            .await
            .unwrap();
        let other = issue_token(&pool, b, None, Duration::minutes(15))
            .await
            .unwrap();
        let new = issue_token(&pool, a, Some(b), Duration::minutes(15))
            .await
            .unwrap();
        assert_eq!(peek_valid(&pool, &old.raw).await.unwrap(), None);
        assert_eq!(consume(&pool, &old.raw).await.unwrap(), None);
        assert_eq!(peek_valid(&pool, &new.raw).await.unwrap(), Some(a));
        assert_eq!(peek_valid(&pool, &other.raw).await.unwrap(), Some(b));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_consume_has_one_winner(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (pool, raw) = (pool.clone(), t.raw.clone());
                tokio::spawn(async move { consume(&pool, &raw).await.unwrap() })
            })
            .collect();
        let mut winners = 0;
        for task in tasks {
            if task.await.unwrap().is_some() {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);
    }
}
