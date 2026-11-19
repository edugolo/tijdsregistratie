//! Sites. Location and configuration are plaintext (needed for the zone check); the address is encrypted.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use crypto::DataKeys;
use domain::{GeoPoint, SiteZone};
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::{RepoError, Result};
use crate::crypto_ext::{open_str, seal_str};

const TABLE: &str = "site";
const ADDRESS_COLUMN: &str = "address";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonePolicy {
    Flag,
    Block,
}

impl ZonePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ZonePolicy::Flag => "flag",
            ZonePolicy::Block => "block",
        }
    }
}

impl FromStr for ZonePolicy {
    type Err = RepoError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "flag" => Ok(ZonePolicy::Flag),
            "block" => Ok(ZonePolicy::Block),
            _ => Err(RepoError::InvalidValue("zone_policy")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub radius_m: f64,
    pub break_minutes: i32,
    pub break_after_minutes: i32,
    pub zone_policy: ZonePolicy,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

/// Editable fields of a site.
#[derive(Debug, Clone, PartialEq)]
pub struct SiteInput {
    pub name: String,
    pub address: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub radius_m: f64,
    pub break_minutes: i32,
    pub break_after_minutes: i32,
    pub zone_policy: ZonePolicy,
    pub active: bool,
}

#[derive(sqlx::FromRow)]
struct SiteRow {
    id: Uuid,
    name: String,
    address_enc: Option<Vec<u8>>,
    lat: Option<f64>,
    lng: Option<f64>,
    radius_m: f32,
    break_minutes: i32,
    break_after_minutes: i32,
    zone_policy: String,
    active: bool,
    created_at: DateTime<Utc>,
}

impl SiteRow {
    fn decrypt(self, keys: &DataKeys) -> Result<Site> {
        let address = self
            .address_enc
            .as_deref()
            .map(|b| open_str(keys, TABLE, ADDRESS_COLUMN, self.id, b))
            .transpose()?;
        Ok(Site {
            id: self.id,
            name: self.name,
            address,
            lat: self.lat,
            lng: self.lng,
            radius_m: f64::from(self.radius_m),
            break_minutes: self.break_minutes,
            break_after_minutes: self.break_after_minutes,
            zone_policy: self.zone_policy.parse()?,
            active: self.active,
            created_at: self.created_at,
        })
    }
}

macro_rules! site_columns {
    () => {
        "id, name, address_enc, lat, lng, radius_m, break_minutes, break_after_minutes, zone_policy, active, created_at"
    };
}

fn seal_address(keys: &DataKeys, id: Uuid, address: Option<&str>) -> Result<Option<Vec<u8>>> {
    Ok(address
        .map(|a| seal_str(keys, TABLE, ADDRESS_COLUMN, id, a))
        .transpose()?)
}

/// Takes a pool or a transaction.
pub async fn create_site<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    input: &SiteInput,
) -> Result<Site> {
    let id = Uuid::new_v4();
    let row: SiteRow = sqlx::query_as(concat!(
        "INSERT INTO site (id, name, address_enc, lat, lng, radius_m, break_minutes, \
         break_after_minutes, zone_policy, active) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ",
        site_columns!(),
        ""
    ))
    .bind(id)
    .bind(&input.name)
    .bind(seal_address(keys, id, input.address.as_deref())?)
    .bind(input.lat)
    .bind(input.lng)
    .bind(input.radius_m as f32)
    .bind(input.break_minutes)
    .bind(input.break_after_minutes)
    .bind(input.zone_policy.as_str())
    .bind(input.active)
    .fetch_one(db)
    .await?;
    row.decrypt(keys)
}

/// Fails with [`RepoError::NotFound`] if the site does not exist. Takes a pool or a transaction.
pub async fn update_site<'e>(
    db: impl PgExecutor<'e>,
    keys: &DataKeys,
    id: Uuid,
    input: &SiteInput,
) -> Result<Site> {
    let row: Option<SiteRow> = sqlx::query_as(concat!(
        "UPDATE site SET name = $2, address_enc = $3, lat = $4, lng = $5, radius_m = $6, \
         break_minutes = $7, break_after_minutes = $8, zone_policy = $9, active = $10 \
         WHERE id = $1 RETURNING ",
        site_columns!(),
        ""
    ))
    .bind(id)
    .bind(&input.name)
    .bind(seal_address(keys, id, input.address.as_deref())?)
    .bind(input.lat)
    .bind(input.lng)
    .bind(input.radius_m as f32)
    .bind(input.break_minutes)
    .bind(input.break_after_minutes)
    .bind(input.zone_policy.as_str())
    .bind(input.active)
    .fetch_optional(db)
    .await?;
    row.ok_or(RepoError::NotFound)?.decrypt(keys)
}

pub async fn get_site(pool: &PgPool, keys: &DataKeys, id: Uuid) -> Result<Option<Site>> {
    sqlx::query_as::<_, SiteRow>(concat!(
        "SELECT ",
        site_columns!(),
        " FROM site WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(|r| r.decrypt(keys))
    .transpose()
}

/// Sites sorted by name.
pub async fn list_sites(pool: &PgPool, keys: &DataKeys, active_only: bool) -> Result<Vec<Site>> {
    sqlx::query_as::<_, SiteRow>(concat!(
        "SELECT ",
        site_columns!(),
        " FROM site WHERE (NOT $1 OR active) ORDER BY lower(name), id"
    ))
    .bind(active_only)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| r.decrypt(keys))
    .collect()
}

/// The zone of the site, or `None` if it has no coordinates yet.
pub fn to_zone(site: &Site) -> Option<SiteZone> {
    Some(SiteZone {
        id: site.id,
        center: GeoPoint {
            lat: site.lat?,
            lng: site.lng?,
        },
        radius_m: site.radius_m,
    })
}
