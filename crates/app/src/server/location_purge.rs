//! Automatic retention purge: shortly after startup and then daily, coordinates and clock notes
//! older than the retention period are removed. The work lives in `store::repo::location_purge`.

use std::time::Duration;

use chrono::Utc;
use sqlx::PgPool;
use store::repo::location_purge::run_location_purge;
use tokio::time::{Instant, MissedTickBehavior};

const FIRST_RUN_AFTER: Duration = Duration::from_secs(60);
const RUN_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Starts the background task. Must be called inside the tokio runtime.
pub fn spawn(pool: PgPool) {
    tokio::spawn(purge_periodically(pool));
}

async fn purge_periodically(pool: PgPool) {
    let mut interval = tokio::time::interval_at(Instant::now() + FIRST_RUN_AFTER, RUN_EVERY);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        purge_once(&pool).await;
    }
}

/// One run. A failure is logged without data and retried at the next interval.
async fn purge_once(pool: &PgPool) {
    match run_location_purge(pool, Utc::now(), None).await {
        Ok(Some(run)) => tracing::info!(
            locations = run.locations,
            notes = run.notes,
            cutoff = %run.cutoff.to_rfc3339(),
            "retention purge done"
        ),
        Ok(None) => tracing::debug!("retention purge skipped: another instance is running it"),
        Err(e) => {
            tracing::warn!(error = %e, "retention purge failed; retrying at the next interval")
        }
    }
}
