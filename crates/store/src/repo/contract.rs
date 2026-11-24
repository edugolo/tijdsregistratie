//! Contract minutes per weekday (`work_schedule`). Not personal data in itself, but only
//! HR and admins change it.

use std::collections::HashMap;

use sqlx::{PgConnection, PgExecutor};
use uuid::Uuid;

use super::Result;

/// Minutes per ISO weekday, Monday first.
pub type Schedule = [u32; 7];

/// Replaces the person's schedule. Takes a connection so it can share the audit transaction.
pub async fn set_schedule(
    conn: &mut PgConnection,
    person_id: Uuid,
    minutes: &Schedule,
) -> Result<()> {
    for (i, m) in minutes.iter().enumerate() {
        sqlx::query(
            "INSERT INTO work_schedule (person_id, weekday, minutes) VALUES ($1, $2, $3) \
             ON CONFLICT (person_id, weekday) DO UPDATE SET minutes = $3",
        )
        .bind(person_id)
        .bind(i as i16 + 1)
        .bind(i32::try_from(*m).unwrap_or(i32::MAX))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Schedules of the given people; people without a contract are absent.
pub async fn schedules<'e>(
    executor: impl PgExecutor<'e>,
    person_ids: &[Uuid],
) -> Result<HashMap<Uuid, Schedule>> {
    let rows: Vec<(Uuid, i16, i32)> = sqlx::query_as(
        "SELECT person_id, weekday, minutes FROM work_schedule WHERE person_id = ANY($1)",
    )
    .bind(person_ids)
    .fetch_all(executor)
    .await?;
    let mut out: HashMap<Uuid, Schedule> = HashMap::new();
    for (person, weekday, minutes) in rows {
        if let Some(slot) = usize::try_from(weekday - 1)
            .ok()
            .and_then(|i| out.entry(person).or_default().get_mut(i))
        {
            *slot = u32::try_from(minutes).unwrap_or(0);
        }
    }
    Ok(out)
}
