//! Free text typed by people: correction reasons and clock notes.

use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

pub const FREE_TEXT_MIN_CHARS: usize = 3;
pub const FREE_TEXT_MAX_CHARS: usize = 500;

/// The trimmed text when it is 3 to 500 characters long and holds no control or format
/// characters (Unicode general category `Cc` except `'\n'`, and `Cf`), `None` otherwise.
/// Format characters include zero-width and bidi controls: invisible text, or text that renders
/// differently on a manager's screen than it was typed.
pub fn free_text(value: &str) -> Option<&str> {
    if value.chars().any(is_refused_char) {
        return None;
    }
    let value = value.trim();
    (FREE_TEXT_MIN_CHARS..=FREE_TEXT_MAX_CHARS)
        .contains(&value.chars().count())
        .then_some(value)
}

fn is_refused_char(c: char) -> bool {
    c != '\n'
        && matches!(
            c.general_category(),
            GeneralCategory::Control | GeneralCategory::Format
        )
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

    #[test]
    fn invisible_and_control_characters_are_refused() {
        for bad in [
            "\u{200B}\u{200B}\u{200B}",
            "abc\u{200B}",
            "abc\u{202E}def",
            "\u{2066}abc\u{2069}",
            "abc\u{FEFF}",
            "abc\u{00AD}def",
            "abc\u{0000}",
            "abc\u{0007}def",
            "abc\tdef",
            "abc\rdef",
            "abc\u{0085}def",
            "abc\u{E0041}",
        ] {
            assert_eq!(free_text(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn newlines_and_ordinary_text_stay_allowed() {
        assert_eq!(
            free_text("uitstap\nmet de klas"),
            Some("uitstap\nmet de klas")
        );
        assert_eq!(free_text("\n abc \n"), Some("abc"));
        assert_eq!(free_text("café ☕ 10 €"), Some("café ☕ 10 €"));
        // Non-breaking space is a separator, not a control character.
        assert_eq!(free_text("a\u{00A0}bc"), Some("a\u{00A0}bc"));
    }

    #[test]
    fn a_long_text_with_a_control_character_is_refused_not_trimmed_to_size() {
        let mut text = "x".repeat(FREE_TEXT_MAX_CHARS + 10);
        text.push('\u{202E}');
        assert_eq!(free_text(&text), None);
    }
}
