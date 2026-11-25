//! Free text typed by people: correction reasons and clock notes.

pub const FREE_TEXT_MIN_CHARS: usize = 3;
pub const FREE_TEXT_MAX_CHARS: usize = 500;

/// The trimmed text when it is 3 to 500 characters long, `None` otherwise.
pub fn free_text(value: &str) -> Option<&str> {
    let value = value.trim();
    (FREE_TEXT_MIN_CHARS..=FREE_TEXT_MAX_CHARS)
        .contains(&value.chars().count())
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_text_is_trimmed_and_bounded() {
        assert_eq!(free_text("  abc "), Some("abc"));
        assert_eq!(free_text("ab"), None);
        assert_eq!(free_text("  a  "), None);
        assert_eq!(free_text(""), None);
        assert!(free_text(&"x".repeat(FREE_TEXT_MAX_CHARS)).is_some());
        assert!(free_text(&"x".repeat(FREE_TEXT_MAX_CHARS + 1)).is_none());
        // Characters, not bytes.
        assert!(free_text(&"é".repeat(FREE_TEXT_MAX_CHARS)).is_some());
        assert_eq!(FREE_TEXT_MIN_CHARS, 3);
    }
}
