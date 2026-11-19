//! Audit log. Records ids and field names only, never values.

use sqlx::PgPool;
use uuid::Uuid;

use super::Result;

pub async fn record(
    pool: &PgPool,
    actor: Option<Uuid>,
    action: &str,
    entity: &str,
    entity_id: Option<Uuid>,
    changed_fields: &[&str],
) -> Result<()> {
    let fields: Vec<String> = changed_fields.iter().map(|f| (*f).to_string()).collect();
    sqlx::query(
        "INSERT INTO audit_log (actor_id, action, entity, entity_id, changed_fields) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(entity)
    .bind(entity_id)
    .bind(fields)
    .execute(pool)
    .await?;
    Ok(())
}
