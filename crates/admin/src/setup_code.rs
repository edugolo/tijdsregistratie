//! `setup-code`: a fresh one-time code for the first-run wizard.

use anyhow::{Result, bail};
use sqlx::PgPool;
use store::repo::setup;

/// Creates a new code (older ones stop working). Refused once setup is completed.
pub async fn issue(pool: &PgPool) -> Result<String> {
    if !setup::setup_required(pool).await? {
        bail!("setup is already completed: an admin with an active passkey exists");
    }
    Ok(setup::new_code(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn issues_a_valid_code_and_invalidates_the_previous_one(pool: PgPool) {
        let first = issue(&pool).await.unwrap();
        let second = issue(&pool).await.unwrap();
        assert!(!setup::code_is_valid(&pool, &first).await.unwrap());
        assert!(setup::code_is_valid(&pool, &second).await.unwrap());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn refused_when_an_admin_has_a_passkey(pool: PgPool) {
        sqlx::query(
            "INSERT INTO person (webauthn_user_handle, display_name_enc) VALUES ('\\x01', '\\x01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO role_assignment (person_id, role) SELECT id, 'admin' FROM person")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO passkey (person_id, credential_id, credential) SELECT id, '\\x02', '{}'::jsonb FROM person")
            .execute(&pool)
            .await
            .unwrap();
        assert!(issue(&pool).await.is_err());
    }
}
