//! One-time enrollment tokens. Raw tokens are never stored; only their SHA-256 hash is.
//!
//! Each token comes with a short code that can be typed instead of scanning the QR code. The
//! code lives on the token's row (only as a hash), so it shares the token's person, expiry and
//! single use: redeeming either consumes both, and a newer token invalidates both.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rand::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::{RepoError, Result};

/// Symbols in a typed enrollment code: 12 of 32 symbols, 60 bits.
pub const CODE_LEN: usize = 12;

/// A freshly issued token. `raw` and `code` are shown once (QR code / link, typed code) and
/// never stored.
#[derive(Clone)]
pub struct IssuedToken {
    /// URL-safe base64 (no padding) of 32 random bytes.
    pub raw: String,
    /// The typed alternative, formatted `XXXX-XXXX-XXXX`.
    pub code: String,
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

/// SHA-256 of a raw token: the only form in which tokens are stored (and kept in ceremony state).
pub fn token_hash(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// The normalised form of a typed code (uppercase, without spaces or dashes), or `None` when it
/// cannot be a code: wrong length or a symbol outside the alphabet.
pub fn parse_code(input: &str) -> Option<String> {
    let normalized = super::setup::normalize_code(input);
    (normalized.len() == CODE_LEN
        && normalized
            .bytes()
            .all(|b| super::setup::ALPHABET.contains(&b)))
    .then_some(normalized)
}

/// SHA-256 of a normalised code: the only form in which codes are stored.
fn code_hash(normalized: &str) -> Vec<u8> {
    Sha256::digest(normalized.as_bytes()).to_vec()
}

/// Issues a single-use token for `person_id`, valid for `ttl`, with its typed code. Earlier
/// unused tokens (and so their codes) of the same person are invalidated. Takes a pool or a
/// transaction.
pub async fn issue_token<'e>(
    db: impl sqlx::PgExecutor<'e>,
    person_id: Uuid,
    issued_by: Option<Uuid>,
    ttl: Duration,
) -> Result<IssuedToken> {
    let mut bytes = [0u8; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| RepoError::InvalidValue("random source"))?;
    let raw = URL_SAFE_NO_PAD.encode(bytes);
    let code = super::setup::random_symbols::<CODE_LEN>()?;
    let issued_at = Utc::now();
    let expires_at = issued_at + ttl;

    // One statement (the old tokens are expired by a data-modifying CTE), so it works on a pool
    // and on the caller's transaction connection alike.
    sqlx::query(
        "WITH expire AS ( \
           UPDATE enrollment_token SET expires_at = now() \
           WHERE person_id = $2 AND used_at IS NULL AND expires_at > now()) \
         INSERT INTO enrollment_token \
           (token_hash, code_hash, person_id, issued_by, expires_at, created_at) \
         VALUES ($1, $6, $2, $3, $4, $5)",
    )
    .bind(token_hash(&raw))
    .bind(person_id)
    .bind(issued_by)
    .bind(expires_at)
    .bind(issued_at)
    .bind(code_hash(&code))
    .execute(db)
    .await?;
    Ok(IssuedToken {
        raw,
        code: super::setup::format_code(&code),
        person_id,
        issued_at,
        expires_at,
    })
}

/// Returns the person the token belongs to if it is unused and not expired, without consuming it.
pub async fn peek_valid(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    peek_valid_by_hash(pool, &token_hash(raw)).await
}

/// Like [`peek_valid`], for a token that is only known by its hash.
pub async fn peek_valid_by_hash(pool: &PgPool, hash: &[u8]) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT person_id FROM enrollment_token \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?)
}

/// The token a typed code belongs to, if both are unused and not expired. Does not consume it.
#[derive(Clone, PartialEq, Eq)]
pub struct CodeMatch {
    pub person_id: Uuid,
    /// The token's hash: what a registration ceremony carries and finally consumes.
    pub token_hash: Vec<u8>,
}

impl std::fmt::Debug for CodeMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeMatch")
            .field("person_id", &self.person_id)
            .finish_non_exhaustive()
    }
}

/// Looks up the valid token of a typed code (any case, spaces and dashes ignored). Malformed
/// input is rejected without a query. Never consumes: the ceremony consumes the token by hash.
pub async fn peek_code(pool: &PgPool, input: &str) -> Result<Option<CodeMatch>> {
    let Some(normalized) = parse_code(input) else {
        return Ok(None);
    };
    let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        "SELECT person_id, token_hash FROM enrollment_token \
         WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(code_hash(&normalized))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(person_id, token_hash)| CodeMatch {
        person_id,
        token_hash,
    }))
}

/// Atomically marks the token used and returns its person, or `None` if it is unknown, used or expired.
pub async fn consume(pool: &PgPool, raw: &str) -> Result<Option<Uuid>> {
    consume_by_hash(pool, &token_hash(raw)).await
}

/// Like [`consume`], for a token that is only known by its hash. Takes a pool or a transaction.
pub async fn consume_by_hash<'e>(
    db: impl sqlx::PgExecutor<'e>,
    hash: &[u8],
) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "UPDATE enrollment_token SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() RETURNING person_id",
    )
    .bind(hash)
    .fetch_optional(db)
    .await?)
}

/// What happens to the person's earlier passkeys when an enrollment completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Devices {
    /// One device per person: earlier passkeys are revoked in the same transaction.
    ReplaceExisting,
    /// Several devices per person: the new passkey is added next to the existing ones.
    AddToExisting,
}

/// Finishes an enrollment in one transaction: consume the token (it must belong to `person_id`),
/// store the passkey (with its non-personal device `label`), revoke earlier passkeys when
/// `devices` says so and write the audit rows. Returns the new passkey id, or `None` (and
/// changes nothing) if the token is unknown, used, expired or belongs to someone else. Any error
/// rolls back, leaving the token unused.
pub async fn complete(
    pool: &PgPool,
    hash: &[u8],
    person_id: Uuid,
    credential_id: &[u8],
    credential_json: serde_json::Value,
    label: Option<&str>,
    devices: Devices,
) -> Result<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    if consume_by_hash(&mut *tx, hash).await? != Some(person_id) {
        return Ok(None);
    }
    let passkey_id =
        super::passkey::insert_labeled(&mut *tx, person_id, credential_id, credential_json, label)
            .await?;
    super::audit::record(
        &mut *tx,
        Some(person_id),
        "passkey.registered",
        "passkey",
        Some(passkey_id),
        &[],
    )
    .await?;
    if devices == Devices::ReplaceExisting {
        let replaced =
            super::passkey::revoke_all_except(&mut *tx, person_id, passkey_id, None).await?;
        for id in replaced {
            super::audit::record(
                &mut *tx,
                Some(person_id),
                "passkey.revoked",
                "passkey",
                Some(id),
                &[],
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Some(passkey_id))
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
    async fn consume_by_hash_matches_raw_token(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        let h = token_hash(&t.raw);
        assert_eq!(peek_valid_by_hash(&pool, &h).await.unwrap(), Some(p));
        assert_eq!(consume_by_hash(&pool, &h).await.unwrap(), Some(p));
        assert_eq!(consume_by_hash(&pool, &h).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_consumes_token_and_stores_passkey_atomically(pool: PgPool) {
        let p = person(&pool).await;
        let t = issue_token(&pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        let h = token_hash(&t.raw);
        let id = complete(
            &pool,
            &h,
            p,
            b"cred-1",
            serde_json::json!({}),
            None,
            Devices::AddToExisting,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), None);
        let stored = super::super::passkey::find_by_credential_id(&pool, b"cred-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.id, id);
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'passkey.registered' AND entity_id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit, 1);
        // Second use: nothing happens.
        assert!(
            complete(
                &pool,
                &h,
                p,
                b"cred-2",
                serde_json::json!({}),
                None,
                Devices::AddToExisting
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_failure_leaves_token_unused(pool: PgPool) {
        let (a, b) = (person(&pool).await, person(&pool).await);
        super::super::passkey::insert(&pool, b, b"dup", serde_json::json!({}))
            .await
            .unwrap();
        let t = issue_token(&pool, a, None, Duration::minutes(15))
            .await
            .unwrap();
        let h = token_hash(&t.raw);
        // Passkey insert fails (credential id taken): the token must not be burnt.
        assert!(
            complete(
                &pool,
                &h,
                a,
                b"dup",
                serde_json::json!({}),
                None,
                Devices::AddToExisting
            )
            .await
            .is_err()
        );
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), Some(a));
        // Token of another person than the ceremony's: rejected, token untouched.
        assert!(
            complete(
                &pool,
                &h,
                b,
                b"fresh",
                serde_json::json!({}),
                None,
                Devices::AddToExisting
            )
            .await
            .unwrap()
            .is_none()
        );
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), Some(a));
        let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audit, 0);
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

    async fn enroll(pool: &PgPool, p: Uuid, cred: &[u8], devices: Devices) -> Uuid {
        let t = issue_token(pool, p, None, Duration::minutes(15))
            .await
            .unwrap();
        complete(
            pool,
            &token_hash(&t.raw),
            p,
            cred,
            serde_json::json!({}),
            Some("Android – Chrome"),
            devices,
        )
        .await
        .unwrap()
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn replacing_enrollment_revokes_the_previous_passkeys(pool: PgPool) {
        let (p, other) = (person(&pool).await, person(&pool).await);
        let other_pk = enroll(&pool, other, b"o1", Devices::ReplaceExisting).await;
        let old = enroll(&pool, p, b"c1", Devices::ReplaceExisting).await;
        let new = enroll(&pool, p, b"c2", Devices::ReplaceExisting).await;
        let active = super::super::passkey::devices_of(&pool, p).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, new);
        assert_eq!(active[0].label.as_deref(), Some("Android – Chrome"));
        let before = Utc::now();
        assert!(
            !super::super::person::session_is_valid(&pool, p, old, before)
                .await
                .unwrap()
        );
        // The new device and other people are untouched.
        assert!(
            super::super::person::session_is_valid(&pool, p, new, Utc::now())
                .await
                .unwrap()
        );
        assert!(
            super::super::person::session_is_valid(&pool, other, other_pk, before)
                .await
                .unwrap()
        );
        let revoked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'passkey.revoked' AND entity_id = $1",
        )
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(revoked, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn adding_enrollment_keeps_the_previous_passkeys(pool: PgPool) {
        let p = person(&pool).await;
        let first = enroll(&pool, p, b"c1", Devices::AddToExisting).await;
        let second = enroll(&pool, p, b"c2", Devices::AddToExisting).await;
        let ids: Vec<Uuid> = super::super::passkey::devices_of(&pool, p)
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(ids, vec![first, second]);
        assert!(
            super::super::person::session_is_valid(&pool, p, first, Utc::now())
                .await
                .unwrap()
        );
    }

    // --- typed code ---

    async fn issued(pool: &PgPool, p: Uuid, ttl: Duration) -> IssuedToken {
        issue_token(pool, p, None, ttl).await.unwrap()
    }

    #[test]
    fn code_parsing_normalises_and_rejects_malformed_input() {
        assert_eq!(
            parse_code("ABCD-EFGH-JKLM").as_deref(),
            Some("ABCDEFGHJKLM")
        );
        assert_eq!(
            parse_code(" abcd efgh-jklm ").as_deref(),
            Some("ABCDEFGHJKLM")
        );
        assert_eq!(parse_code("abcdefghjklm").as_deref(), Some("ABCDEFGHJKLM"));
        for bad in [
            "",
            "ABCD-EFGH-JKL",
            "ABCD-EFGH-JKLMN",
            "ABCD-EFGH-JKL0",
            "ABCD-EFGH-JKLO",
            "ABCD-EFGH-JKL1",
            "ABCD-EFGH-JKLI",
        ] {
            assert_eq!(parse_code(bad), None, "{bad}");
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn code_has_format_and_only_its_hash_is_stored(pool: PgPool) {
        let p = person(&pool).await;
        let t = issued(&pool, p, Duration::minutes(15)).await;
        assert_eq!(t.code.len(), 14);
        let groups: Vec<&str> = t.code.split('-').collect();
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| {
            g.len() == 4
                && g.bytes()
                    .all(|b| super::super::setup::ALPHABET.contains(&b))
        }));
        let stored: Vec<u8> = sqlx::query_scalar("SELECT code_hash FROM enrollment_token")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored.len(), 32);
        assert_eq!(stored, code_hash(&parse_code(&t.code).unwrap()));
        assert!(!format!("{t:?}").contains(&t.code), "Debug hides the code");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn code_leads_to_the_same_token_in_any_spelling(pool: PgPool) {
        let p = person(&pool).await;
        let t = issued(&pool, p, Duration::minutes(15)).await;
        let expected = CodeMatch {
            person_id: p,
            token_hash: token_hash(&t.raw),
        };
        for input in [
            t.code.clone(),
            t.code.to_lowercase(),
            t.code.replace('-', " "),
            t.code.replace('-', ""),
        ] {
            assert_eq!(
                peek_code(&pool, &input).await.unwrap(),
                Some(expected.clone())
            );
        }
        // Peeking does not consume.
        assert_eq!(peek_valid(&pool, &t.raw).await.unwrap(), Some(p));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn wrong_and_expired_codes_find_nothing(pool: PgPool) {
        let p = person(&pool).await;
        issued(&pool, p, Duration::minutes(15)).await;
        assert_eq!(peek_code(&pool, "AAAA-AAAA-AAAA").await.unwrap(), None);
        assert_eq!(peek_code(&pool, "nonsense").await.unwrap(), None);
        let expired = issued(&pool, p, Duration::seconds(-5)).await;
        assert_eq!(peek_code(&pool, &expired.code).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn code_and_token_share_single_use(pool: PgPool) {
        let p = person(&pool).await;
        // Redeemed through the code (the ceremony consumes the token it points to).
        let a = issued(&pool, p, Duration::minutes(15)).await;
        let m = peek_code(&pool, &a.code).await.unwrap().unwrap();
        assert_eq!(
            consume_by_hash(&pool, &m.token_hash).await.unwrap(),
            Some(p)
        );
        assert_eq!(peek_code(&pool, &a.code).await.unwrap(), None);
        assert_eq!(
            peek_valid(&pool, &a.raw).await.unwrap(),
            None,
            "QR link is used too"
        );
        // Redeemed through the QR link: the code is used too.
        let b = issued(&pool, p, Duration::minutes(15)).await;
        assert_eq!(consume(&pool, &b.raw).await.unwrap(), Some(p));
        assert_eq!(peek_code(&pool, &b.code).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn newer_token_invalidates_the_old_code_of_the_same_person_only(pool: PgPool) {
        let (a, b) = (person(&pool).await, person(&pool).await);
        let old = issued(&pool, a, Duration::minutes(15)).await;
        let other = issued(&pool, b, Duration::minutes(15)).await;
        let new = issued(&pool, a, Duration::minutes(15)).await;
        assert_ne!(old.code, new.code);
        assert_eq!(peek_code(&pool, &old.code).await.unwrap(), None);
        assert_eq!(
            peek_code(&pool, &new.code)
                .await
                .unwrap()
                .map(|m| m.person_id),
            Some(a)
        );
        assert_eq!(
            peek_code(&pool, &other.code)
                .await
                .unwrap()
                .map(|m| m.person_id),
            Some(b)
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn completing_with_the_codes_token_hash_registers_once(pool: PgPool) {
        let p = person(&pool).await;
        let t = issued(&pool, p, Duration::minutes(15)).await;
        let m = peek_code(&pool, &t.code).await.unwrap().unwrap();
        let args = |cred: &'static [u8]| (cred, serde_json::json!({}));
        let (cred, json) = args(b"code-1");
        assert!(
            complete(
                &pool,
                &m.token_hash,
                p,
                cred,
                json,
                None,
                Devices::AddToExisting
            )
            .await
            .unwrap()
            .is_some()
        );
        let (cred, json) = args(b"code-2");
        assert!(
            complete(
                &pool,
                &m.token_hash,
                p,
                cred,
                json,
                None,
                Devices::AddToExisting
            )
            .await
            .unwrap()
            .is_none()
        );
    }
}
