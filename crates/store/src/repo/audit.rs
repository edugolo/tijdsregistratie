//! Audit log. Records ids and field names only, never values.

use sqlx::PgExecutor;
use uuid::Uuid;

use super::Result;

/// Appends an audit row. Pass `&mut *tx` to write it in the same transaction as the mutation it
/// describes, or `&pool` for a standalone row.
pub async fn record<'e>(
    executor: impl PgExecutor<'e>,
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
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn audit_log_is_append_only(pool: PgPool) {
        record(&pool, None, "x", "person", None, &[]).await.unwrap();
        assert!(
            sqlx::query("UPDATE audit_log SET action = 'y'")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM audit_log")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("TRUNCATE audit_log")
                .execute(&pool)
                .await
                .is_err()
        );
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn record_in_a_rolled_back_transaction_leaves_nothing(pool: PgPool) {
        let mut tx = pool.begin().await.unwrap();
        record(&mut *tx, None, "x", "person", None, &[])
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
