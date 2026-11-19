//! Pure domain logic: zone check, day totals and break deduction.
//!
//! This crate performs no IO, no database access and never reads the clock; callers pass time in.

/// Name of the crate, used as a trivial smoke-test target until real logic lands.
pub const CRATE_NAME: &str = "domain";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_is_domain() {
        assert_eq!(CRATE_NAME, "domain");
    }
}
