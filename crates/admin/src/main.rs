//! Operator CLI: bootstrap the first admin, seed sites, purge coordinates, rotate the data key.

use clap::{Parser, Subcommand};

// Dependencies for the subcommands that are still stubs; remove as each one lands.
use {qrcode as _, store as _};

#[derive(Parser)]
#[command(
    name = "admin",
    version,
    about = "Operator CLI for the time registration app"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the first admin and print an enrollment QR code
    BootstrapAdmin,
    /// Seed sites from a TOML file
    SeedSites,
    /// Purge stored coordinates older than the retention period
    PurgeLocations,
    /// Re-encrypt personal data with a new data key
    RotateKey,
}

fn main() {
    match Cli::parse().command {
        Command::BootstrapAdmin
        | Command::SeedSites
        | Command::PurgeLocations
        | Command::RotateKey => println!("not implemented yet"),
    }
}
