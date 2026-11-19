//! Organisation settings (single row, id = 1).

use chrono_tz::Tz;
use domain::ZoneConfig;
use sqlx::PgPool;

use super::{RepoError, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct OrgSettings {
    pub display_name: String,
    /// Short name for the app header and the installed app.
    pub short_name: String,
    pub has_logo: bool,
    pub brand_color: Option<String>,
    pub retention_days: i32,
    pub zone: ZoneConfig,
    /// Time zone in which a day starts and ends.
    pub timezone: Tz,
}

/// Reads the settings. A plain read: the default row is inserted by the migrations.
pub async fn get_settings(pool: &PgPool) -> Result<OrgSettings> {
    let (
        display_name,
        short_name,
        has_logo,
        brand_color,
        retention_days,
        max_accuracy_m,
        tolerance_m,
        timezone,
    ): (
        String,
        String,
        bool,
        Option<String>,
        i32,
        f32,
        f32,
        String,
    ) = sqlx::query_as(
        "SELECT display_name, short_name, logo IS NOT NULL, brand_color, location_retention_days, max_accuracy_m, tolerance_m, timezone \
         FROM organisation_settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;
    let timezone = timezone
        .parse()
        .map_err(|_| RepoError::InvalidValue("timezone"))?;
    Ok(OrgSettings {
        display_name,
        short_name,
        has_logo,
        brand_color,
        retention_days,
        zone: ZoneConfig {
            max_accuracy_m: f64::from(max_accuracy_m),
            tolerance_m: f64::from(tolerance_m),
        },
        timezone,
    })
}

/// The organisation fields an admin can edit (setup wizard and A0).
#[derive(Debug, Clone, PartialEq)]
pub struct OrgProfile {
    pub display_name: String,
    pub short_name: String,
    pub brand_color: String,
    pub timezone: Tz,
}

/// Saves the organisation profile, inserting the settings row first if it is missing.
pub async fn update_profile(pool: &PgPool, profile: &OrgProfile) -> Result<()> {
    sqlx::query(
        "INSERT INTO organisation_settings (id, display_name, short_name, brand_color, timezone) \
         VALUES (1, $1, $2, $3, $4) \
         ON CONFLICT (id) DO UPDATE SET display_name = $1, short_name = $2, brand_color = $3, \
         timezone = $4, updated_at = now()",
    )
    .bind(&profile.display_name)
    .bind(&profile.short_name)
    .bind(&profile.brand_color)
    .bind(profile.timezone.name())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_settings_only_reads(pool: PgPool) {
        // The migration inserts the row; reading must not (re)create it.
        sqlx::query("DELETE FROM organisation_settings")
            .execute(&pool)
            .await
            .unwrap();
        assert!(get_settings(&pool).await.is_err());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM organisation_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
