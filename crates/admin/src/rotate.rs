//! Key rotation: re-encrypts blobs that are not on the current key version, in batches.
//!
//! `time_event.location_enc` is deliberately not rotated: the table is append-only (a trigger
//! only allows nulling the location) and locations are purged after the retention period.

use anyhow::Result;
use crypto::{Aad, DataKeys};
use sqlx::PgPool;
use uuid::Uuid;

/// (table, column, aad column name) of every rotatable encrypted field.
const FIELDS: [(&str, &str, &str); 3] = [
    ("person", "display_name_enc", "display_name"),
    ("person", "email_enc", "email"),
    ("site", "address_enc", "address"),
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RotateReport {
    pub person_display_name: u64,
    pub person_email: u64,
    pub site_address: u64,
}

pub async fn rotate(pool: &PgPool, keys: &DataKeys, batch: i64) -> Result<RotateReport> {
    let mut counts = [0u64; 3];
    for (i, (table, column, aad_column)) in FIELDS.into_iter().enumerate() {
        counts[i] = rotate_field(pool, keys, table, column, aad_column, batch).await?;
    }
    Ok(RotateReport {
        person_display_name: counts[0],
        person_email: counts[1],
        site_address: counts[2],
    })
}

async fn rotate_field(
    pool: &PgPool,
    keys: &DataKeys,
    table: &str,
    column: &str,
    aad_column: &str,
    batch: i64,
) -> Result<u64> {
    // table and column come from the constant list above, never from input
    let select = format!(
        "SELECT id, {column} FROM {table} \
         WHERE {column} IS NOT NULL AND get_byte({column}, 0) <> $1 \
         ORDER BY id LIMIT $2 FOR UPDATE"
    );
    let update = format!("UPDATE {table} SET {column} = $2 WHERE id = $1");
    let current = i32::from(keys.current_version());
    let mut total = 0;
    loop {
        let mut tx = pool.begin().await?;
        let rows: Vec<(Uuid, Vec<u8>)> = sqlx::query_as(sqlx::AssertSqlSafe(select.as_str()))
            .bind(current)
            .bind(batch)
            .fetch_all(&mut *tx)
            .await?;
        if rows.is_empty() {
            return Ok(total);
        }
        for (id, blob) in &rows {
            let aad = Aad::new(table, aad_column, *id)?;
            let fresh = keys.reencrypt(blob, &aad)?;
            sqlx::query(sqlx::AssertSqlSafe(update.as_str()))
                .bind(id)
                .bind(fresh)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        total += rows.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{BlindIndexKey, generate_key_line};
    use store::crypto_ext::{open_str, seal_str};
    use store::repo::person;
    use store::repo::site::{self, SiteInput, ZonePolicy};

    fn site_input() -> SiteInput {
        SiteInput {
            name: "Voorbeeldschool".into(),
            address: Some("Voorbeeldstraat 1".into()),
            lat: Some(51.0),
            lng: Some(4.0),
            radius_m: 100.0,
            break_minutes: 30,
            break_after_minutes: 360,
            zone_policy: ZonePolicy::Flag,
            active: true,
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rotates_only_old_blobs_and_they_still_decrypt(pool: PgPool) {
        let line1 = generate_key_line(1);
        let line2 = generate_key_line(2);
        let k1 = DataKeys::parse(&line1).unwrap();
        let both = DataKeys::parse(&format!("{line1}\n{line2}")).unwrap();
        let k2_only = DataKeys::parse(&line2).unwrap();
        let b = BlindIndexKey::parse(&generate_key_line(1)).unwrap();

        // Old person and site (v1), a person with e-mail (v1), and one already on v2.
        let old = person::create_person(&pool, &k1, &b, "Oud Persoon", None)
            .await
            .unwrap();
        let email = seal_str(&k1, "person", "email", old.id, "oud@example.org").unwrap();
        sqlx::query("UPDATE person SET email_enc = $2 WHERE id = $1")
            .bind(old.id)
            .bind(&email)
            .execute(&pool)
            .await
            .unwrap();
        site::create_site(&pool, &k1, &site_input()).await.unwrap();
        let fresh = person::create_person(&pool, &both, &b, "Nieuw Persoon", None)
            .await
            .unwrap();
        let fresh_blob: Vec<u8> =
            sqlx::query_scalar("SELECT display_name_enc FROM person WHERE id = $1")
                .bind(fresh.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fresh_blob[0], 2);

        let report = rotate(&pool, &both, 1).await.unwrap();
        assert_eq!(
            report,
            RotateReport {
                person_display_name: 1,
                person_email: 1,
                site_address: 1
            }
        );

        // Everything is on v2: readable with the v2 key alone; the already-new blob is untouched.
        let blob: Vec<u8> = sqlx::query_scalar("SELECT display_name_enc FROM person WHERE id = $1")
            .bind(old.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(blob[0], 2);
        let got = person::get_person(&pool, &k2_only, old.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.display_name, "Oud Persoon");
        let email_blob: Vec<u8> = sqlx::query_scalar("SELECT email_enc FROM person WHERE id = $1")
            .bind(old.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            open_str(&k2_only, "person", "email", old.id, &email_blob).unwrap(),
            "oud@example.org"
        );
        let sites = site::list_sites(&pool, &k2_only, false).await.unwrap();
        assert_eq!(sites[0].address.as_deref(), Some("Voorbeeldstraat 1"));
        let after: Vec<u8> =
            sqlx::query_scalar("SELECT display_name_enc FROM person WHERE id = $1")
                .bind(fresh.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after, fresh_blob);

        // Second run has nothing left to do.
        assert_eq!(
            rotate(&pool, &both, 100).await.unwrap(),
            RotateReport::default()
        );
    }
}
