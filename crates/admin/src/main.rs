//! Operator CLI: bootstrap the first admin, seed sites, purge coordinates, rotate the data key.
//!
//! Errors never include key material or personal data.

mod bootstrap;
mod rotate;
mod setup_code;
mod sites;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand};
use crypto::{BlindIndexKey, DataKeys};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use store::repo::{person, settings, setup, time_event};
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "admin",
    version,
    about = "Operator CLI for the time registration app",
    after_help = "Environment: DATABASE_URL, DATA_KEY_FILE, BLIND_INDEX_KEY_FILE, APP_ORIGIN (bootstrap-admin only)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a new key line `vN:<base64>` (redirect to a file with mode 0600)
    GenerateKey {
        /// Key version, 1..=255
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..))]
        version: u8,
    },
    /// Create the first admin and print an enrollment link and QR code
    BootstrapAdmin {
        /// Display name of the admin
        #[arg(long)]
        name: String,
        /// Create another admin even if an active one exists
        #[arg(long)]
        force: bool,
    },
    /// Create or update sites from a TOML file (matched by exact name)
    SeedSites {
        #[arg(long)]
        file: PathBuf,
    },
    /// Purge stored coordinates older than the retention period
    PurgeLocations {
        /// Cutoff (RFC 3339); default: now minus the configured retention days
        #[arg(long)]
        before: Option<DateTime<Utc>>,
    },
    /// Re-encrypt personal data with the newest key in the key file
    RotateKey,
    /// List admins (ids and names)
    ListAdmins,
    /// Print a fresh setup code for the first-run wizard (invalidates older codes)
    SetupCode,
}

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("environment variable {name} is not set"))
}

async fn connect() -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&env("DATABASE_URL")?)
        .await
        .context("cannot connect to the database")?;
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("cannot run migrations")?;
    Ok(pool)
}

fn data_keys() -> Result<DataKeys> {
    DataKeys::from_file(env("DATA_KEY_FILE")?).context("cannot load DATA_KEY_FILE")
}

fn blind_key() -> Result<BlindIndexKey> {
    BlindIndexKey::from_file(env("BLIND_INDEX_KEY_FILE")?)
        .context("cannot load BLIND_INDEX_KEY_FILE")
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse().command).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::GenerateKey { version } => {
            println!("{}", crypto::generate_key_line(version));
        }
        Command::BootstrapAdmin { name, force } => {
            let name = name.trim();
            if name.is_empty() {
                bail!("--name must not be empty");
            }
            let origin = env("APP_ORIGIN")?;
            let (keys, blind) = (data_keys()?, blind_key()?);
            let pool = connect().await?;
            let (id, url) =
                bootstrap::bootstrap(&pool, &keys, &blind, &origin, name, force).await?;
            println!("Admin created: {id}");
            println!("Enrollment link (valid 30 minutes, single use):\n{url}\n");
            println!("{}", bootstrap::render_qr(&url)?);
        }
        Command::SeedSites { file } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let inputs = sites::parse_sites(&text)?;
            let keys = data_keys()?;
            let pool = connect().await?;
            let r = sites::seed(&pool, &keys, &inputs).await?;
            println!(
                "Sites: {} created, {} updated, {} unchanged",
                r.created, r.updated, r.unchanged
            );
        }
        Command::PurgeLocations { before } => {
            let pool = connect().await?;
            let cutoff = match before {
                Some(t) => t,
                None => {
                    let days = settings::get_settings(&pool).await?.retention_days;
                    Utc::now() - Duration::days(i64::from(days))
                }
            };
            let n = time_event::purge_locations(&pool, cutoff).await?;
            println!("Purged {n} locations older than {}", cutoff.to_rfc3339());
        }
        Command::RotateKey => {
            let keys = data_keys()?;
            let pool = connect().await?;
            let r = rotate::rotate(&pool, &keys, 200).await?;
            println!("Re-encrypted to key version {}:", keys.current_version());
            println!("  person display names: {}", r.person_display_name);
            println!("  person e-mail addresses: {}", r.person_email);
            println!("  site addresses: {}", r.site_address);
            println!(
                "Not rotated: time_event locations (append-only). Keep older key versions in \
                 the key file until all locations written with them have been purged."
            );
        }
        Command::SetupCode => {
            let pool = connect().await?;
            println!("Setup code: {}", setup_code::issue(&pool).await?);
            println!("Valid for {} hours, single use.", setup::CODE_TTL_HOURS);
        }
        Command::ListAdmins => {
            let keys = data_keys()?;
            let pool = connect().await?;
            let ids: Vec<Uuid> = sqlx::query_scalar(
                "SELECT DISTINCT person_id FROM role_assignment WHERE role = 'admin'",
            )
            .fetch_all(&pool)
            .await?;
            let mut admins = Vec::new();
            for id in ids {
                if let Some(p) = person::get_person(&pool, &keys, id).await? {
                    admins.push(p);
                }
            }
            admins.sort_by_key(|p| p.created_at);
            for p in admins {
                let state = if p.active { "active" } else { "inactive" };
                println!("{}\t{}\t{}", p.id, state, p.display_name);
            }
        }
    }
    Ok(())
}
