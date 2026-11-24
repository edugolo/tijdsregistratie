//! API keys of the public REST API. Only the SHA-256 hash of a key is stored; the key itself is
//! returned once, at creation. A key is `tr_live_` followed by 43 random base62 characters
//! (about 256 bits).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use rand::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::{RepoError, Result};

pub const KEY_PREFIX: &str = "tr_live_";
const RANDOM_CHARS: usize = 43;
const DISPLAY_CHARS: usize = 8;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// `last_used_at` is refreshed at most this often per key.
const LAST_USED_GRANULARITY: Duration = Duration::minutes(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiScope {
    PeopleRead,
    PeopleWrite,
    HoursRead,
    ExportRead,
}

impl ApiScope {
    pub const ALL: [ApiScope; 4] = [
        ApiScope::PeopleRead,
        ApiScope::PeopleWrite,
        ApiScope::HoursRead,
        ApiScope::ExportRead,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ApiScope::PeopleRead => "people:read",
            ApiScope::PeopleWrite => "people:write",
            ApiScope::HoursRead => "hours:read",
            ApiScope::ExportRead => "export:read",
        }
    }
}

impl FromStr for ApiScope {
    type Err = RepoError;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == s)
            .ok_or(RepoError::InvalidValue("api scope"))
    }
}

/// A key as stored: no secret in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub id: Uuid,
    pub name: String,
    /// Display prefix, like `tr_live_Ab3dEf9h`.
    pub prefix: String,
    pub scopes: Vec<ApiScope>,
    /// `None`: all sites.
    pub site_ids: Option<Vec<Uuid>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<Uuid>,
}

impl ApiKey {
    pub fn has_scope(&self, scope: ApiScope) -> bool {
        self.scopes.contains(&scope)
    }

    /// Whether the key may see `site` (always true for an unrestricted key).
    pub fn allows_site(&self, site: Uuid) -> bool {
        self.site_ids.as_ref().is_none_or(|ids| ids.contains(&site))
    }
}

/// A freshly created key. `raw` is shown once and never stored.
#[derive(Clone)]
pub struct CreatedKey {
    pub key: ApiKey,
    pub raw: String,
}

impl fmt::Debug for CreatedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedKey")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

pub struct NewKey<'a> {
    pub name: &'a str,
    pub scopes: &'a [ApiScope],
    pub site_ids: Option<&'a [Uuid]>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
}

/// SHA-256 of a raw key: the only form in which keys are stored.
pub fn key_hash(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

/// Whether `raw` has the shape of a key (cheap check before any database lookup).
pub fn looks_like_key(raw: &str) -> bool {
    raw.strip_prefix(KEY_PREFIX)
        .is_some_and(|rest| rest.len() == RANDOM_CHARS && rest.bytes().all(|b| BASE62.contains(&b)))
}

/// A new random key. Characters are drawn by rejection sampling, so they are uniform.
pub fn generate_key() -> Result<String> {
    let mut key = String::with_capacity(KEY_PREFIX.len() + RANDOM_CHARS);
    key.push_str(KEY_PREFIX);
    let mut byte = [0u8; 1];
    let mut chars = 0;
    while chars < RANDOM_CHARS {
        SysRng
            .try_fill_bytes(&mut byte)
            .map_err(|_| RepoError::InvalidValue("random source"))?;
        // 248 = 4 * 62: the largest multiple of 62 that fits in a byte.
        if byte[0] < 248 {
            key.push(BASE62[usize::from(byte[0] % 62)] as char);
            chars += 1;
        }
    }
    Ok(key)
}

fn display_prefix(raw: &str) -> String {
    raw.chars().take(KEY_PREFIX.len() + DISPLAY_CHARS).collect()
}

#[derive(sqlx::FromRow)]
struct KeyRow {
    id: Uuid,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    site_ids: Option<Vec<Uuid>>,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    revoked_by: Option<Uuid>,
}

impl KeyRow {
    fn into_key(self) -> Result<ApiKey> {
        Ok(ApiKey {
            scopes: self
                .scopes
                .iter()
                .map(|s| s.parse())
                .collect::<Result<_>>()?,
            id: self.id,
            name: self.name,
            prefix: self.prefix,
            site_ids: self.site_ids,
            created_by: self.created_by,
            created_at: self.created_at,
            expires_at: self.expires_at,
            last_used_at: self.last_used_at,
            revoked_at: self.revoked_at,
            revoked_by: self.revoked_by,
        })
    }
}

macro_rules! columns {
    () => {
        "id, name, prefix, scopes, site_ids, created_by, created_at, expires_at, \
         last_used_at, revoked_at, revoked_by"
    };
}

/// Creates a key. Takes a pool or a transaction.
pub async fn create<'e>(db: impl PgExecutor<'e>, new: &NewKey<'_>) -> Result<CreatedKey> {
    let raw = generate_key()?;
    let scopes: Vec<&str> = new.scopes.iter().map(|s| s.as_str()).collect();
    let row = sqlx::query_as::<_, KeyRow>(concat!(
        "INSERT INTO api_key (name, prefix, key_hash, scopes, site_ids, created_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ",
        columns!()
    ))
    .bind(new.name)
    .bind(display_prefix(&raw))
    .bind(key_hash(&raw))
    .bind(scopes)
    .bind(new.site_ids)
    .bind(new.created_by)
    .bind(new.expires_at)
    .fetch_one(db)
    .await?;
    Ok(CreatedKey {
        key: row.into_key()?,
        raw,
    })
}

/// All keys, newest first, revoked ones included. Never contains a secret.
pub async fn list(pool: &PgPool) -> Result<Vec<ApiKey>> {
    sqlx::query_as::<_, KeyRow>(concat!(
        "SELECT ",
        columns!(),
        " FROM api_key ORDER BY created_at DESC, id"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(KeyRow::into_key)
    .collect()
}

/// Revokes a key. Returns false when it does not exist or was already revoked.
pub async fn revoke<'e>(db: impl PgExecutor<'e>, id: Uuid, by: Option<Uuid>) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE api_key SET revoked_at = now(), revoked_by = $2 \
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The usable key behind a raw key: known, not revoked and not expired at `now`. Looks the
/// key up by its hash and refreshes `last_used_at` at most once per minute.
pub async fn verify(pool: &PgPool, raw: &str, now: DateTime<Utc>) -> Result<Option<ApiKey>> {
    if !looks_like_key(raw) {
        return Ok(None);
    }
    let row = sqlx::query_as::<_, KeyRow>(concat!(
        "SELECT ",
        columns!(),
        " FROM api_key \
         WHERE key_hash = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > $2)"
    ))
    .bind(key_hash(raw))
    .bind(now)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    if row
        .last_used_at
        .is_none_or(|at| now - at >= LAST_USED_GRANULARITY)
    {
        sqlx::query(
            "UPDATE api_key SET last_used_at = $2 \
             WHERE id = $1 AND (last_used_at IS NULL OR last_used_at <= $3)",
        )
        .bind(row.id)
        .bind(now)
        .bind(now - LAST_USED_GRANULARITY)
        .execute(pool)
        .await?;
    }
    Ok(Some(row.into_key()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(name: &'a str, scopes: &'a [ApiScope]) -> NewKey<'a> {
        NewKey {
            name,
            scopes,
            site_ids: None,
            expires_at: None,
            created_by: None,
        }
    }

    #[test]
    fn generated_keys_have_the_documented_shape() {
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("tr_live_"));
        assert_eq!(a.len(), "tr_live_".len() + 43);
        assert!(looks_like_key(&a));
        assert!(!looks_like_key("tr_live_short"));
        assert!(!looks_like_key(&a.replace("tr_live_", "tr_test_")));
        assert!(!looks_like_key(&format!("{}-", &a[..a.len() - 1])));
    }

    #[test]
    fn hash_is_sha256_of_the_key() {
        let h = key_hash("abc");
        assert_eq!(h.len(), 32);
        assert_eq!(
            h[..4],
            [0xba, 0x78, 0x16, 0xbf],
            "SHA-256 of abc starts with ba7816bf"
        );
    }

    #[test]
    fn scopes_parse_and_reject_unknown() {
        for scope in ApiScope::ALL {
            assert_eq!(scope.as_str().parse::<ApiScope>().unwrap(), scope);
        }
        assert!("people:delete".parse::<ApiScope>().is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_stores_only_the_hash_and_verify_finds_it(pool: PgPool) {
        let created = create(&pool, &new("Payroll", &[ApiScope::PeopleRead]))
            .await
            .unwrap();
        assert_eq!(created.key.prefix, display_prefix(&created.raw));
        assert_eq!(created.key.prefix.len(), 16);
        let stored: Vec<u8> = sqlx::query_scalar("SELECT key_hash FROM api_key")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, key_hash(&created.raw));
        let found = verify(&pool, &created.raw, Utc::now())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, created.key.id);
        assert!(found.has_scope(ApiScope::PeopleRead));
        assert!(!found.has_scope(ApiScope::HoursRead));
        let wrong = generate_key().unwrap();
        assert!(verify(&pool, &wrong, Utc::now()).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn revoked_and_expired_keys_do_not_verify(pool: PgPool) {
        let now = Utc::now();
        let revoked = create(&pool, &new("a", &[ApiScope::PeopleRead]))
            .await
            .unwrap();
        assert!(revoke(&pool, revoked.key.id, None).await.unwrap());
        assert!(!revoke(&pool, revoked.key.id, None).await.unwrap());
        assert!(verify(&pool, &revoked.raw, now).await.unwrap().is_none());

        let mut n = new("b", &[ApiScope::PeopleRead]);
        n.expires_at = Some(now + Duration::days(1));
        let expiring = create(&pool, &n).await.unwrap();
        assert!(verify(&pool, &expiring.raw, now).await.unwrap().is_some());
        assert!(
            verify(&pool, &expiring.raw, now + Duration::days(2))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn last_used_is_refreshed_at_most_once_per_minute(pool: PgPool) {
        let created = create(&pool, &new("a", &[ApiScope::PeopleRead]))
            .await
            .unwrap();
        let t0 = Utc::now();
        verify(&pool, &created.raw, t0).await.unwrap();
        verify(&pool, &created.raw, t0 + Duration::seconds(30))
            .await
            .unwrap();
        let used = |pool: &PgPool| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<DateTime<Utc>>>("SELECT last_used_at FROM api_key")
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        assert_eq!(used(&pool).await.timestamp_micros(), t0.timestamp_micros());
        let t1 = t0 + Duration::seconds(61);
        verify(&pool, &created.raw, t1).await.unwrap();
        assert_eq!(used(&pool).await.timestamp_micros(), t1.timestamp_micros());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_has_no_secrets_and_site_restriction_round_trips(pool: PgPool) {
        let site = Uuid::new_v4();
        let sites = [site];
        let mut n = new("Site key", &[ApiScope::PeopleRead, ApiScope::HoursRead]);
        n.site_ids = Some(&sites);
        let created = create(&pool, &n).await.unwrap();
        let keys = list(&pool).await.unwrap();
        assert_eq!(keys, vec![created.key.clone()]);
        assert!(keys[0].allows_site(site));
        assert!(!keys[0].allows_site(Uuid::new_v4()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn database_rejects_unknown_or_empty_scopes(pool: PgPool) {
        for scopes in [Vec::new(), vec!["people:delete".to_string()]] {
            let r = sqlx::query(
                "INSERT INTO api_key (name, prefix, key_hash, scopes) \
                 VALUES ('x', 'p', $1, $2)",
            )
            .bind(Uuid::new_v4().as_bytes().to_vec())
            .bind(&scopes)
            .execute(&pool)
            .await;
            assert!(r.is_err(), "{scopes:?} must be refused");
        }
    }
}
