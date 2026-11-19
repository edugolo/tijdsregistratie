//! Organisation settings (single row, id = 1).

use domain::ZoneConfig;
use sqlx::PgPool;

use super::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct OrgSettings {
    pub display_name: String,
    pub brand_color: Option<String>,
    pub retention_days: i32,
    pub zone: ZoneConfig,
}

/// Reads the settings, inserting the default row first if it is missing.
pub async fn get_settings(pool: &PgPool) -> Result<OrgSettings> {
    sqlx::query("INSERT INTO organisation_settings (id) VALUES (1) ON CONFLICT DO NOTHING")
        .execute(pool)
        .await?;
    let (display_name, brand_color, retention_days, max_accuracy_m, tolerance_m): (
        String,
        Option<String>,
        i32,
        f32,
        f32,
    ) = sqlx::query_as(
        "SELECT display_name, brand_color, location_retention_days, max_accuracy_m, tolerance_m \
         FROM organisation_settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;
    Ok(OrgSettings {
        display_name,
        brand_color,
        retention_days,
        zone: ZoneConfig {
            max_accuracy_m: f64::from(max_accuracy_m),
            tolerance_m: f64::from(tolerance_m),
        },
    })
}
