//! First-run setup: setup mode detection, one-time setup codes and the single transaction that
//! creates the organisation, the first site and the first admin.
//!
//! Setup codes are stored as SHA-256 hashes only and are valid for 24 hours.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use crypto::{BlindIndexKey, DataKeys, normalize_name};
use rand::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::settings::OrgProfile;
use super::site::SiteInput;
use super::{RepoError, Result};
use crate::crypto_ext::seal_str;

/// How long a setup code stays valid.
pub const CODE_TTL_HOURS: i64 = 24;
/// Unambiguous alphabet (no 0/O, 1/I): 32 symbols, so a random byte maps without bias.
pub(super) const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 16;

/// Uppercases and drops everything that is not a letter or digit (dashes, spaces).
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `ABCDEFGHJKLMNPQR` becomes `ABCD-EFGH-JKLM-NPQR`.
pub fn format_code(normalized: &str) -> String {
    normalized
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

fn code_hash(input: &str) -> Vec<u8> {
    Sha256::digest(normalize_code(input).as_bytes()).to_vec()
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| RepoError::InvalidValue("random source"))?;
    Ok(bytes)
}

/// Setup mode: `setup_completed_at` is unset and no admin has ever had a passkey. The database
/// sets the marker at the first admin passkey and never clears it, so once false this stays
/// false: losing devices, revoking passkeys or deactivating admins does not re-open setup.
macro_rules! setup_required_expr {
    () => {
        "COALESCE((SELECT setup_completed_at IS NULL FROM organisation_settings WHERE id = 1), true) \
         AND NOT EXISTS (SELECT 1 FROM passkey pk \
         JOIN role_assignment ra ON ra.person_id = pk.person_id AND ra.role = 'admin')"
    };
}
const SETUP_REQUIRED_SQL: &str = concat!("SELECT ", setup_required_expr!());

/// Setup mode (see [`SETUP_REQUIRED_SQL`]). A plain read.
pub async fn setup_required(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(SETUP_REQUIRED_SQL)
        .fetch_one(pool)
        .await?)
}

/// Creates a fresh code (formatted `XXXX-XXXX-XXXX-XXXX`) and invalidates all unused older ones.
pub async fn new_code(pool: &PgPool) -> Result<String> {
    let mut tx = pool.begin().await?;
    let code = insert_new_code(&mut tx).await?;
    tx.commit().await?;
    Ok(code)
}

/// `N` random symbols of [`ALPHABET`]; `b & 31` maps a random byte without bias.
pub(super) fn random_symbols<const N: usize>() -> Result<String> {
    Ok(random_bytes::<N>()?
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 31)]))
        .collect())
}

async fn insert_new_code(tx: &mut Transaction<'_, Postgres>) -> Result<String> {
    let normalized = random_symbols::<CODE_LEN>()?;
    sqlx::query("UPDATE setup_code SET used_at = now() WHERE used_at IS NULL")
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO setup_code (code_hash) VALUES ($1)")
        .bind(code_hash(&normalized))
        .execute(&mut **tx)
        .await?;
    Ok(format_code(&normalized))
}

/// `$1` is [`CODE_TTL_HOURS`].
macro_rules! usable_code_expr {
    () => {
        "EXISTS (SELECT 1 FROM setup_code WHERE \
         (used_at IS NULL AND created_at > now() - make_interval(hours => $1)) \
         OR used_at > now() - interval '30 minutes')"
    };
}

/// Makes sure a usable setup code exists while setup is required and returns a new code when it
/// had to create one (the caller logs it). One statement sequence under an advisory lock, so
/// concurrent callers create exactly one code. Does nothing once setup is completed.
pub async fn announce_code_if_needed(pool: &PgPool) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1952)")
        .execute(&mut *tx)
        .await?;
    let needed: bool = sqlx::query_scalar(concat!(
        "SELECT ",
        setup_required_expr!(),
        " AND NOT ",
        usable_code_expr!()
    ))
    .bind(CODE_TTL_HOURS as i32)
    .fetch_one(&mut *tx)
    .await?;
    if !needed {
        return Ok(None);
    }
    let code = insert_new_code(&mut tx).await?;
    tx.commit().await?;
    Ok(Some(code))
}

/// Whether a code can still be shown to the operator: one is unused and unexpired, or setup
/// finished a moment ago and the new admin is still registering a passkey.
pub async fn has_usable_code(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(concat!("SELECT ", usable_code_expr!()))
        .bind(CODE_TTL_HOURS as i32)
        .fetch_one(pool)
        .await?)
}

/// Whether `code` is unused and not expired. Does not consume it.
pub async fn code_is_valid(pool: &PgPool, code: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM setup_code WHERE code_hash = $1 AND used_at IS NULL \
         AND created_at > now() - make_interval(hours => $2))",
    )
    .bind(code_hash(code))
    .bind(CODE_TTL_HOURS as i32)
    .fetch_one(pool)
    .await?)
}

/// Validated input of [`complete`].
#[derive(Debug, Clone, PartialEq)]
pub struct SetupData {
    pub profile: OrgProfile,
    pub site: SiteInput,
    pub admin_name: String,
    pub also_manager: bool,
}

/// Result of [`complete`]. `token` is the raw enrollment token for the new admin.
#[derive(Clone)]
pub struct Completed {
    pub person_id: Uuid,
    pub site_id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for Completed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Completed")
            .field("person_id", &self.person_id)
            .field("site_id", &self.site_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("setup is already completed")]
    NotRequired,
    #[error("invalid setup code")]
    InvalidCode,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

impl From<sqlx::Error> for SetupError {
    fn from(e: sqlx::Error) -> Self {
        Self::Repo(e.into())
    }
}

impl From<crypto::CryptoError> for SetupError {
    fn from(e: crypto::CryptoError) -> Self {
        Self::Repo(e.into())
    }
}

async fn audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    action: &str,
    entity: &str,
    entity_id: Option<Uuid>,
    fields: &[&str],
) -> std::result::Result<(), sqlx::Error> {
    let fields: Vec<String> = fields.iter().map(|f| (*f).to_string()).collect();
    sqlx::query(
        "INSERT INTO audit_log (actor_id, action, entity, entity_id, changed_fields) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(entity)
    .bind(entity_id)
    .bind(fields)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// In one transaction: consumes the setup code, saves the organisation profile, creates the
/// first site and the admin (also manager of the site when asked) and issues a 30-minute
/// enrollment token. Fails with [`SetupError::NotRequired`] outside setup mode and with
/// [`SetupError::InvalidCode`] for an unknown, used or expired code; nothing is written then.
pub async fn complete(
    pool: &PgPool,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    code: &str,
    data: &SetupData,
    token_ttl: Duration,
) -> std::result::Result<Completed, SetupError> {
    let mut tx = pool.begin().await?;
    // Serialise concurrent setups; the lock is released at commit or rollback.
    sqlx::query("SELECT pg_advisory_xact_lock(1951)")
        .execute(&mut *tx)
        .await?;
    if !sqlx::query_scalar::<_, bool>(SETUP_REQUIRED_SQL)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(SetupError::NotRequired);
    }
    let used = sqlx::query(
        "UPDATE setup_code SET used_at = now() WHERE code_hash = $1 AND used_at IS NULL \
         AND created_at > now() - make_interval(hours => $2)",
    )
    .bind(code_hash(code))
    .bind(CODE_TTL_HOURS as i32)
    .execute(&mut *tx)
    .await?;
    if used.rows_affected() != 1 {
        return Err(SetupError::InvalidCode);
    }

    let p = &data.profile;
    sqlx::query(
        "INSERT INTO organisation_settings (id, display_name, short_name, brand_color, timezone) \
         VALUES (1, $1, $2, $3, $4) \
         ON CONFLICT (id) DO UPDATE SET display_name = $1, short_name = $2, brand_color = $3, \
         timezone = $4, updated_at = now()",
    )
    .bind(&p.display_name)
    .bind(&p.short_name)
    .bind(&p.brand_color)
    .bind(p.timezone.name())
    .execute(&mut *tx)
    .await?;

    let site_id = Uuid::new_v4();
    let s = &data.site;
    let address_enc = s
        .address
        .as_deref()
        .map(|a| seal_str(keys, "site", "address", site_id, a))
        .transpose()?;
    sqlx::query(
        "INSERT INTO site (id, name, address_enc, lat, lng, radius_m, break_minutes, \
         break_after_minutes, zone_policy, active) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(site_id)
    .bind(&s.name)
    .bind(address_enc)
    .bind(s.lat)
    .bind(s.lng)
    .bind(s.radius_m as f32)
    .bind(s.break_minutes)
    .bind(s.break_after_minutes)
    .bind(s.zone_policy.as_str())
    .bind(s.active)
    .execute(&mut *tx)
    .await?;

    let person_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO person (id, webauthn_user_handle, display_name_enc, name_index) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(person_id)
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .bind(seal_str(
        keys,
        "person",
        "display_name",
        person_id,
        &data.admin_name,
    )?)
    .bind(blind.index(&normalize_name(&data.admin_name)).to_vec())
    .execute(&mut *tx)
    .await?;
    let assign = "INSERT INTO role_assignment (person_id, role, site_id, created_by) \
                  VALUES ($1, $2, $3, $1)";
    sqlx::query(assign)
        .bind(person_id)
        .bind("admin")
        .bind(None::<Uuid>)
        .execute(&mut *tx)
        .await?;
    if data.also_manager {
        sqlx::query(assign)
            .bind(person_id)
            .bind("manager")
            .bind(Some(site_id))
            .execute(&mut *tx)
            .await?;
    }

    let raw = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
    let issued_at = Utc::now();
    let expires_at = issued_at + token_ttl;
    sqlx::query(
        "INSERT INTO enrollment_token (token_hash, person_id, issued_by, expires_at, created_at) \
         VALUES ($1, $2, NULL, $3, $4)",
    )
    .bind(Sha256::digest(raw.as_bytes()).to_vec())
    .bind(person_id)
    .bind(expires_at)
    .bind(issued_at)
    .execute(&mut *tx)
    .await?;

    let actor = Some(person_id);
    audit_tx(
        &mut tx,
        actor,
        "settings.updated",
        "organisation_settings",
        None,
        &["display_name", "short_name", "brand_color", "timezone"],
    )
    .await?;
    audit_tx(&mut tx, actor, "site.created", "site", Some(site_id), &[]).await?;
    audit_tx(
        &mut tx,
        actor,
        "person.created",
        "person",
        Some(person_id),
        &[],
    )
    .await?;
    audit_tx(
        &mut tx,
        actor,
        "role.assigned",
        "person",
        Some(person_id),
        &["role"],
    )
    .await?;
    if data.also_manager {
        audit_tx(
            &mut tx,
            actor,
            "role.assigned",
            "person",
            Some(person_id),
            &["role", "site"],
        )
        .await?;
    }
    audit_tx(
        &mut tx,
        actor,
        "setup.completed",
        "organisation_settings",
        None,
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(Completed {
        person_id,
        site_id,
        token: raw,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::site::ZonePolicy;
    use crate::repo::{enrollment, role};
    use chrono_tz::Tz;
    use crypto::generate_key_line;

    fn keys() -> (DataKeys, BlindIndexKey) {
        (
            DataKeys::parse(&generate_key_line(1)).unwrap(),
            BlindIndexKey::parse(&generate_key_line(1)).unwrap(),
        )
    }

    fn data(also_manager: bool) -> SetupData {
        SetupData {
            profile: OrgProfile {
                display_name: "Voorbeeldschool Zuid".into(),
                short_name: "Zuid".into(),
                brand_color: "#1e3a8a".into(),
                timezone: Tz::Europe__Brussels,
            },
            site: SiteInput {
                name: "Campus Voorbeeld".into(),
                address: Some("Voorbeeldstraat 1".into()),
                lat: Some(51.0),
                lng: Some(4.0),
                radius_m: 150.0,
                break_minutes: 30,
                break_after_minutes: 360,
                zone_policy: ZonePolicy::Flag,
                active: true,
            },
            admin_name: "Test Admin".into(),
            also_manager,
        }
    }

    async fn add_admin_passkey(pool: &PgPool, person: Uuid, revoked: bool) {
        sqlx::query(
            "INSERT INTO passkey (person_id, credential_id, credential, revoked_at) \
             VALUES ($1, $2, '{}'::jsonb, CASE WHEN $3 THEN now() END)",
        )
        .bind(person)
        .bind(Uuid::new_v4().as_bytes().to_vec())
        .bind(revoked)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn code_normalizing_and_formatting() {
        assert_eq!(normalize_code(" abcd-efgh jklm_npqr "), "ABCDEFGHJKLMNPQR");
        assert_eq!(format_code("ABCDEFGHJKLMNPQR"), "ABCD-EFGH-JKLM-NPQR");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn generated_code_has_format_and_is_stored_hashed(pool: PgPool) {
        let code = new_code(&pool).await.unwrap();
        assert_eq!(code.len(), 19);
        assert!(
            code.split('-')
                .all(|g| g.len() == 4 && g.bytes().all(|b| ALPHABET.contains(&b)))
        );
        let stored: Vec<u8> = sqlx::query_scalar("SELECT code_hash FROM setup_code")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored.len(), 32);
        assert!(code_is_valid(&pool, &code).await.unwrap());
        // Lowercase and without dashes also works.
        assert!(
            code_is_valid(&pool, &code.replace('-', "").to_lowercase())
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn new_code_invalidates_older_ones(pool: PgPool) {
        let old = new_code(&pool).await.unwrap();
        let new = new_code(&pool).await.unwrap();
        assert!(!code_is_valid(&pool, &old).await.unwrap());
        assert!(code_is_valid(&pool, &new).await.unwrap());
        assert!(!code_is_valid(&pool, "AAAA-AAAA-AAAA-AAAA").await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn expired_code_is_invalid(pool: PgPool) {
        let code = new_code(&pool).await.unwrap();
        sqlx::query("UPDATE setup_code SET created_at = now() - interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!code_is_valid(&pool, &code).await.unwrap());
        assert!(!has_usable_code(&pool).await.unwrap());
        let (k, b) = keys();
        let r = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30)).await;
        assert!(matches!(r, Err(SetupError::InvalidCode)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn setup_required_ends_at_the_first_admin_passkey_and_never_returns(pool: PgPool) {
        assert!(setup_required(&pool).await.unwrap());
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        // An admin without passkey does not end setup mode.
        assert!(setup_required(&pool).await.unwrap());
        // Any passkey ever registered by an admin does, even one that is revoked later.
        add_admin_passkey(&pool, done.person_id, true).await;
        assert!(!setup_required(&pool).await.unwrap());
        let completed: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT setup_completed_at FROM organisation_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(completed.is_some(), "recorded by the database itself");
        // Losing every device, deactivating or deleting the admin does not reopen setup.
        sqlx::query("UPDATE passkey SET revoked_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE person SET active = false")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!setup_required(&pool).await.unwrap());
        sqlx::query("DELETE FROM passkey")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!setup_required(&pool).await.unwrap());
        // The marker cannot be cleared again.
        sqlx::query("UPDATE organisation_settings SET setup_completed_at = NULL")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!setup_required(&pool).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn passkey_of_a_non_admin_does_not_end_setup(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        sqlx::query("DELETE FROM role_assignment WHERE person_id = $1")
            .bind(done.person_id)
            .execute(&pool)
            .await
            .unwrap();
        add_admin_passkey(&pool, done.person_id, false).await;
        assert!(setup_required(&pool).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn migration_backfills_installations_that_already_have_an_admin_passkey(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        add_admin_passkey(&pool, done.person_id, false).await;
        // Recreate the state of an installation from before the trigger existed.
        sqlx::raw_sql(
            "ALTER TABLE organisation_settings DISABLE TRIGGER USER; \
             UPDATE organisation_settings SET setup_completed_at = NULL; \
             ALTER TABLE organisation_settings ENABLE TRIGGER USER;",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../migrations/20260401000000_setup_never_reopens.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let completed: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT setup_completed_at FROM organisation_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(completed.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn settings_row_exists_after_migration(pool: PgPool) {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM organisation_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn announce_creates_one_code_even_when_called_concurrently(pool: PgPool) {
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let pool = pool.clone();
                tokio::spawn(async move { announce_code_if_needed(&pool).await })
            })
            .collect();
        let mut created = Vec::new();
        for t in tasks {
            created.extend(t.await.unwrap().unwrap());
        }
        assert_eq!(created.len(), 1, "exactly one caller logs a code");
        assert!(code_is_valid(&pool, &created[0]).await.unwrap());
        assert_eq!(announce_code_if_needed(&pool).await.unwrap(), None);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn announce_is_refused_once_setup_is_completed(pool: PgPool) {
        sqlx::query("UPDATE organisation_settings SET setup_completed_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(announce_code_if_needed(&pool).await.unwrap(), None);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM setup_code")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_is_refused_when_setup_is_marked_completed(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        sqlx::query(
            "UPDATE organisation_settings SET setup_completed_at = now(), display_name = 'Bestaand'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let r = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30)).await;
        assert!(matches!(r, Err(SetupError::NotRequired)));
        let name: String = sqlx::query_scalar("SELECT display_name FROM organisation_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Bestaand", "existing organisation untouched");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn complete_creates_everything(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(true), Duration::minutes(30))
            .await
            .unwrap();
        let s = crate::repo::settings::get_settings(&pool).await.unwrap();
        assert_eq!(s.display_name, "Voorbeeldschool Zuid");
        assert_eq!(s.short_name, "Zuid");
        assert_eq!(s.brand_color.as_deref(), Some("#1e3a8a"));
        let site = crate::repo::site::get_site(&pool, &k, done.site_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(site.name, "Campus Voorbeeld");
        assert_eq!(site.address.as_deref(), Some("Voorbeeldstraat 1"));
        assert!(role::is_admin(&pool, done.person_id).await.unwrap());
        assert_eq!(
            role::managed_sites(&pool, done.person_id).await.unwrap(),
            vec![done.site_id]
        );
        assert_eq!(
            enrollment::peek_valid(&pool, &done.token).await.unwrap(),
            Some(done.person_id)
        );
        assert!(!code_is_valid(&pool, &code).await.unwrap());
        let actions: Vec<String> =
            sqlx::query_scalar("SELECT action FROM audit_log ORDER BY at, action")
                .fetch_all(&pool)
                .await
                .unwrap();
        for a in [
            "settings.updated",
            "site.created",
            "person.created",
            "setup.completed",
        ] {
            assert!(actions.iter().any(|x| x == a), "missing audit {a}");
        }
        assert_eq!(actions.iter().filter(|a| *a == "role.assigned").count(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn without_manager_flag_no_manager_role(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        assert!(
            role::managed_sites(&pool, done.person_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn wrong_or_used_code_writes_nothing(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let wrong = complete(
            &pool,
            &k,
            &b,
            "AAAA-AAAA-AAAA-AAAA",
            &data(false),
            Duration::minutes(30),
        )
        .await;
        assert!(matches!(wrong, Err(SetupError::InvalidCode)));
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        let again = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30)).await;
        assert!(matches!(again, Err(SetupError::InvalidCode)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn refused_once_an_admin_has_a_passkey(pool: PgPool) {
        let (k, b) = keys();
        let code = new_code(&pool).await.unwrap();
        let done = complete(&pool, &k, &b, &code, &data(false), Duration::minutes(30))
            .await
            .unwrap();
        add_admin_passkey(&pool, done.person_id, false).await;
        let code2 = new_code(&pool).await.unwrap();
        let r = complete(&pool, &k, &b, &code2, &data(false), Duration::minutes(30)).await;
        assert!(matches!(r, Err(SetupError::NotRequired)));
        assert!(
            code_is_valid(&pool, &code2).await.unwrap(),
            "code not consumed"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn usable_code_logic(pool: PgPool) {
        assert!(!has_usable_code(&pool).await.unwrap());
        new_code(&pool).await.unwrap();
        assert!(has_usable_code(&pool).await.unwrap());
    }
}
