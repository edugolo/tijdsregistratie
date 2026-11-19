//! People. The display name is stored encrypted; `name_index` is a blind index for exact lookups.

use std::fmt;

use chrono::{DateTime, Utc};
use crypto::{BlindIndexKey, DataKeys, normalize_name};
use sqlx::PgPool;
use uuid::Uuid;

use super::Result;
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "person";
const NAME_COLUMN: &str = "display_name";

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

pub async fn create_person(
    pool: &PgPool,
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
    .fetch_one(pool)
    .await?;
    row.decrypt(keys)
}

pub async fn get_person(pool: &PgPool, keys: &DataKeys, id: Uuid) -> Result<Option<Person>> {
    sqlx::query_as::<_, PersonRow>(concat!(
        "SELECT ",
        person_columns!(),
        " FROM person WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|r| r.decrypt(keys))
    .transpose()
}

pub async fn get_person_by_user_handle(
    pool: &PgPool,
    keys: &DataKeys,
    handle: &[u8],
) -> Result<Option<Person>> {
    sqlx::query_as::<_, PersonRow>(concat!(
        "SELECT ",
        person_columns!(),
        " FROM person WHERE webauthn_user_handle = $1"
    ))
    .bind(handle)
    .fetch_optional(pool)
    .await?
    .map(|r| r.decrypt(keys))
    .transpose()
}

/// Returns false if the person does not exist. Deactivating also ends all open sessions of the
/// person (they stay ended when the person is activated again).
pub async fn set_active(pool: &PgPool, id: Uuid, active: bool) -> Result<bool> {
    let done = sqlx::query(
        "UPDATE person SET active = $2, \
         sessions_valid_after = CASE WHEN $2 THEN sessions_valid_after ELSE now() END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(active)
    .execute(pool)
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
pub async fn is_active(pool: &PgPool, id: Uuid) -> Result<bool> {
    let active: Option<bool> = sqlx::query_scalar("SELECT active FROM person WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
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
