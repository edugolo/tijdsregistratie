//! People. The display name is stored encrypted; `name_index` is a blind index for exact lookups.

use std::fmt;

use chrono::{DateTime, Utc};
use crypto::{BlindIndexKey, DataKeys, normalize_name};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::Result;
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "person";
const NAME_COLUMN: &str = "display_name";
const PAYROLL_COLUMN: &str = "payroll_number";

#[derive(Clone, PartialEq, Eq)]
pub struct Person {
    pub id: Uuid,
    pub display_name: String,
    pub active: bool,
    pub user_handle: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

impl fmt::Debug for Person {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Person")
            .field("id", &self.id)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    id: Uuid,
    webauthn_user_handle: Vec<u8>,
    display_name_enc: Vec<u8>,
    active: bool,
    created_at: DateTime<Utc>,
}

impl PersonRow {
    fn decrypt(self, keys: &DataKeys) -> Result<Person> {
        Ok(Person {
            display_name: open_str(keys, TABLE, NAME_COLUMN, self.id, &self.display_name_enc)?,
            id: self.id,
            active: self.active,
            user_handle: self.webauthn_user_handle,
            created_at: self.created_at,
        })
    }
}

macro_rules! person_columns {
    () => {
        "id, webauthn_user_handle, display_name_enc, active, created_at"
    };
}

fn decrypt_sorted(rows: Vec<PersonRow>, keys: &DataKeys) -> Result<Vec<Person>> {
    let mut people = rows
        .into_iter()
        .map(|r| r.decrypt(keys))
        .collect::<Result<Vec<_>>>()?;
    people.sort_by_cached_key(|p| (p.display_name.to_lowercase(), p.id));
    Ok(people)
}

/// Takes a pool or a transaction.
pub async fn create_person<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    display_name: &str,
    created_by: Option<Uuid>,
) -> Result<Person> {
    let id = Uuid::new_v4();
    // WebAuthn user handle: the 16 bytes of a random v4 UUID (what webauthn-rs expects as user
    // id), never derived from the person's id or data.
    let handle = Uuid::new_v4().as_bytes().to_vec();
    let enc = seal_str(keys, TABLE, NAME_COLUMN, id, display_name)?;
    let index = blind.index(&normalize_name(display_name)).to_vec();
    let row: PersonRow = sqlx::query_as(concat!(
        "INSERT INTO person (id, webauthn_user_handle, display_name_enc, name_index, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING ",
        person_columns!(),
        ""
    ))
    .bind(id)
    .bind(handle)
    .bind(enc)
    .bind(index)
    .bind(created_by)
    .fetch_one(db)
    .await?;
    row.decrypt(keys)
}

pub async fn get_person<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    id: Uuid,
) -> Result<Option<Person>> {
    sqlx::query_as::<_, PersonRow>(concat!(
        "SELECT ",
        person_columns!(),
        " FROM person WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?
    .map(|r| r.decrypt(keys))
    .transpose()
}

pub async fn get_person_by_user_handle<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    handle: &[u8],
) -> Result<Option<Person>> {
    sqlx::query_as::<_, PersonRow>(concat!(
        "SELECT ",
        person_columns!(),
        " FROM person WHERE webauthn_user_handle = $1"
    ))
    .bind(handle)
    .fetch_optional(db)
    .await?
    .map(|r| r.decrypt(keys))
    .transpose()
}

/// Returns false if the person does not exist. Takes a pool or a transaction. Deactivating also ends all open sessions of the
/// person (they stay ended when the person is activated again).
pub async fn set_active<'e>(db: impl PgExecutor<'e>, id: Uuid, active: bool) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE person SET active = $2, \
         sessions_valid_after = CASE WHEN $2 THEN sessions_valid_after ELSE clock_timestamp() END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(active)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Whether a session of `person_id` that signed in at `authenticated_at` with `passkey_id` may
/// still be used: the person is active, no revocation or deactivation happened since, and the
/// passkey belongs to the person and is not revoked.
pub async fn session_is_valid(
    pool: &PgPool,
    person_id: Uuid,
    passkey_id: Uuid,
    authenticated_at: DateTime<Utc>,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM person p JOIN passkey k ON k.person_id = p.id \
         WHERE p.id = $1 AND k.id = $2 AND p.active AND k.revoked_at IS NULL \
         AND p.sessions_valid_after <= $3)",
    )
    .bind(person_id)
    .bind(passkey_id)
    .bind(authenticated_at)
    .fetch_one(pool)
    .await?)
}

/// Whether the person exists and is active (no decryption needed).
pub async fn is_active<'e>(db: impl PgExecutor<'e>, id: Uuid) -> Result<bool> {
    let active: Option<bool> = sqlx::query_scalar("SELECT active FROM person WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(active.unwrap_or(false))
}

/// Employees of any of `site_ids`, sorted by name.
pub async fn list_people_for_sites(
    pool: &PgPool,
    keys: &DataKeys,
    site_ids: &[Uuid],
) -> Result<Vec<Person>> {
    if site_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, PersonRow>(
        "SELECT p.id, p.webauthn_user_handle, p.display_name_enc, p.active, p.created_at \
         FROM person p WHERE p.id IN \
         (SELECT person_id FROM role_assignment WHERE role = 'employee' AND site_id = ANY($1))",
    )
    .bind(site_ids)
    .fetch_all(pool)
    .await?;
    decrypt_sorted(rows, keys)
}

pub async fn list_all_people(pool: &PgPool, keys: &DataKeys) -> Result<Vec<Person>> {
    let rows =
        sqlx::query_as::<_, PersonRow>(concat!("SELECT ", person_columns!(), " FROM person"))
            .fetch_all(pool)
            .await?;
    decrypt_sorted(rows, keys)
}

/// Blind index of a payroll number: trimmed and lowercased, with its own prefix so it can never
/// collide with a name index.
fn payroll_index(blind: &BlindIndexKey, number: &str) -> Vec<u8> {
    blind
        .index(&format!("payroll\u{1f}{}", number.trim().to_lowercase()))
        .to_vec()
}

/// Sets or clears (`None`) the payroll number, stored encrypted with a blind index. Returns
/// `false` without changing anything if another person already has that number (compared
/// case-insensitively); the caller must then abandon its transaction, since the database has
/// rejected a statement. Fails with `NotFound` if the person does not exist.
pub async fn set_payroll_number<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    blind: &BlindIndexKey,
    id: Uuid,
    number: Option<&str>,
) -> Result<bool> {
    let enc = number
        .map(|n| seal_str(keys, TABLE, PAYROLL_COLUMN, id, n.trim()))
        .transpose()?;
    let index = number.map(|n| payroll_index(blind, n));
    let result = sqlx::query(
        "UPDATE person SET payroll_number_enc = $2, payroll_number_index = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(enc)
    .bind(index)
    .execute(db)
    .await;
    match result {
        Ok(done) if done.rows_affected() == 0 => Err(super::RepoError::NotFound),
        Ok(_) => Ok(true),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// The decrypted payroll numbers of the given people (people without one are absent). Never log
/// the result.
pub async fn payroll_numbers<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(
        "SELECT id, payroll_number_enc FROM person \
         WHERE id = ANY($1) AND payroll_number_enc IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(|(id, blob)| Ok((id, open_str(keys, TABLE, PAYROLL_COLUMN, id, &blob)?)))
        .collect()
}

/// The person with this payroll number, for API lookups.
pub async fn find_by_payroll_number<'e>(
    db: impl PgExecutor<'e>,
    blind: &BlindIndexKey,
    number: &str,
) -> Result<Option<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM person WHERE payroll_number_index = $1")
            .bind(payroll_index(blind, number))
            .fetch_optional(db)
            .await?,
    )
}
