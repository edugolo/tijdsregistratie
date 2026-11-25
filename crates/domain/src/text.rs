//! Free text typed by people: correction reasons and clock notes.

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

pub const FREE_TEXT_MIN_CHARS: usize = 3;
pub const FREE_TEXT_MAX_CHARS: usize = 500;
/// The most combining marks (`Mn`/`Me`) allowed in a row after one base character.
pub const FREE_TEXT_MAX_MARKS: usize = 4;

const ZWNJ: char = '\u{200C}';
const ZWJ: char = '\u{200D}';
const VS15: char = '\u{FE0E}';
const VS16: char = '\u{FE0F}';
const BLACK_FLAG: char = '\u{1F3F4}';
const TAG_SPECS: std::ops::RangeInclusive<char> = '\u{E0020}'..='\u{E007E}';
const CANCEL_TAG: char = '\u{E007F}';

/// Why a free text was refused, so a client can tell the person what to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeTextError {
    /// Fewer than [`FREE_TEXT_MIN_CHARS`] visible characters.
    TooShort,
    /// More than [`FREE_TEXT_MAX_CHARS`] characters in total.
    TooLong,
    /// An invisible, bidi or control character, or a misplaced joiner, selector or tag.
    InvalidCharacters,
}

/// The trimmed text when it is acceptable, `None` otherwise. See [`check_free_text`].
pub fn free_text(value: &str) -> Option<&str> {
    check_free_text(value).ok()
}

/// Checks a correction reason or clock note and returns it trimmed.
///
/// Surrounding whitespace (spaces, tabs, CR/LF) is trimmed first. The rest is refused with
/// [`FreeTextError::InvalidCharacters`] when it holds:
/// - a control character other than `'\n'` (a `'\r'` is fine right before `'\n'`); a tab inside
///   the text is refused, not rewritten, so the stored text is exactly what was checked;
/// - a bidi control, zero-width space, word joiner or invisible operator, BOM, soft hyphen,
///   Mongolian vowel separator, combining grapheme joiner, Hangul filler or braille blank;
/// - a ZWJ or ZWNJ that is not between two non-space characters;
/// - a variation selector other than VS15/VS16 right after a non-space base character (so
///   `U+FE00`–`U+FE0D`, the supplement `U+E0100`–`U+E01EF` and the Mongolian free variation
///   selectors are always refused);
/// - a tag character outside a flag sequence (`U+1F3F4`, tags, `U+E007F`);
/// - any other format character (`Cf`);
/// - a combining mark with no base character, or more than [`FREE_TEXT_MAX_MARKS`] in a row.
///
/// Otherwise the text is [`FreeTextError::TooLong`] above [`FREE_TEXT_MAX_CHARS`] characters,
/// and [`FreeTextError::TooShort`] with fewer than [`FREE_TEXT_MIN_CHARS`] visible characters
/// (letters, numbers, punctuation and symbols).
pub fn check_free_text(value: &str) -> Result<&str, FreeTextError> {
    let value = value.trim();
    let chars: Vec<char> = value.chars().collect();
    if !chars_allowed(&chars) {
        return Err(FreeTextError::InvalidCharacters);
    }
    if chars.len() > FREE_TEXT_MAX_CHARS {
        return Err(FreeTextError::TooLong);
    }
    if chars.iter().filter(|&&c| is_visible(c)).count() < FREE_TEXT_MIN_CHARS {
        return Err(FreeTextError::TooShort);
    }
    Ok(value)
}

fn chars_allowed(chars: &[char]) -> bool {
    let mut marks = 0;
    let mut in_tags = false;
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        if in_tags {
            match c {
                c if TAG_SPECS.contains(&c) => {}
                CANCEL_TAG => in_tags = false,
                _ => return false,
            }
            continue;
        }
        let allowed = match c {
            '\n' => true,
            '\r' => next == Some('\n'),
            ZWJ | ZWNJ => prev.is_some_and(is_joinable) && next.is_some_and(is_joinable),
            VS15 | VS16 => prev.is_some_and(is_base),
            c if TAG_SPECS.contains(&c) && prev == Some(BLACK_FLAG) => {
                in_tags = true;
                true
            }
            c => !is_refused(c),
        };
        if !allowed {
            return false;
        }
        if is_combining(c) {
            marks += 1;
            if i == 0 || marks > FREE_TEXT_MAX_MARKS {
                return false;
            }
        } else {
            marks = 0;
        }
    }
    !in_tags
}

/// Characters that are refused wherever they stand (outside the exceptions in `chars_allowed`).
fn is_refused(c: char) -> bool {
    is_invisible(c)
        || matches!(
            c.general_category(),
            GeneralCategory::Control | GeneralCategory::Format
        )
}

/// Invisible or look-alike-blank characters that Unicode files under other categories, and the
/// format characters that are named explicitly so the rule does not depend on `Cf` alone.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

fn is_combining(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark
    )
}

/// A neighbour a ZWJ or ZWNJ may stand next to.
fn is_joinable(c: char) -> bool {
    !c.is_whitespace() && c != ZWJ && c != ZWNJ
}

/// A character VS15/VS16 may follow.
fn is_base(c: char) -> bool {
    !c.is_whitespace() && !is_combining(c) && !is_refused(c)
}

fn is_visible(c: char) -> bool {
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Letter
            | GeneralCategoryGroup::Number
            | GeneralCategoryGroup::Punctuation
            | GeneralCategoryGroup::Symbol
    ) && !is_invisible(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use FreeTextError::*;

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
    fn accepted_texts_come_back_trimmed() {
        for (input, expected) in [
            ("uitstap met de klas", "uitstap met de klas"),
            (
                "uitstap met de klas 👩\u{200D}🏫",
                "uitstap met de klas 👩\u{200D}🏫",
            ),
            (
                "🤷\u{200D}♀\u{FE0F} trein had vertraging",
                "🤷\u{200D}♀\u{FE0F} trein had vertraging",
            ),
            ("🏳\u{FE0F}\u{200D}🌈 optocht", "🏳\u{FE0F}\u{200D}🌈 optocht"),
            (
                "👨\u{200D}👩\u{200D}👧\u{200D}👦",
                "👨\u{200D}👩\u{200D}👧\u{200D}👦",
            ),
            (
                "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F} op uitstap",
                "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F} op uitstap",
            ),
            ("می\u{200C}خواهم", "می\u{200C}خواهم"),
            ("क्\u{200D}षत्रिय", "क्\u{200D}षत्रिय"),
            ("café", "café"),
            ("cafe\u{0301}", "cafe\u{0301}"),
            ("1\u{FE0F}\u{20E3} keer", "1\u{FE0F}\u{20E3} keer"),
            ("café ☕ 10 €", "café ☕ 10 €"),
            ("uitstap\nmet de klas", "uitstap\nmet de klas"),
            ("uitstap\r\nmet de klas", "uitstap\r\nmet de klas"),
            ("\n abc \n", "abc"),
            ("\t abc\t", "abc"),
            ("\r\nabc\r\n", "abc"),
            // Non-breaking space is a separator, not a control character.
            ("a\u{00A0}bc", "a\u{00A0}bc"),
            // Four combining marks on one base are still fine.
            (
                "a\u{0300}\u{0301}\u{0302}\u{0303}bc",
                "a\u{0300}\u{0301}\u{0302}\u{0303}bc",
            ),
        ] {
            assert_eq!(check_free_text(input), Ok(expected), "{input:?}");
        }
    }

    #[test]
    fn refused_texts_say_why() {
        let zalgo = format!("a{}bc", "\u{0300}".repeat(5));
        for (input, expected) in [
            ("", TooShort),
            ("ab", TooShort),
            ("  a  ", TooShort),
            ("\u{200B}\u{200B}\u{200B}", InvalidCharacters),
            ("abc\u{200B}", InvalidCharacters),
            ("abc\u{202E}def", InvalidCharacters),
            ("abc\u{200F}def", InvalidCharacters),
            ("abc\u{061C}def", InvalidCharacters),
            ("\u{2066}abc\u{2069}", InvalidCharacters),
            ("ab\u{2068}c", InvalidCharacters),
            ("abc\u{FEFF}", InvalidCharacters),
            ("\u{FEFF}abc", InvalidCharacters),
            ("abc\u{00AD}def", InvalidCharacters),
            ("abc\u{2060}def", InvalidCharacters),
            ("abc\u{180E}def", InvalidCharacters),
            ("abc\u{034F}def", InvalidCharacters),
            ("\u{200D}", InvalidCharacters),
            ("\u{200D}abc", InvalidCharacters),
            ("abc\u{200D}", InvalidCharacters),
            ("ab \u{200D}c", InvalidCharacters),
            ("ab\u{200D}\u{200D}c", InvalidCharacters),
            ("ab\u{200C}\u{200D}c", InvalidCharacters),
            ("\u{3164}\u{3164}\u{3164}", InvalidCharacters),
            ("abc\u{3164}", InvalidCharacters),
            ("\u{115F}\u{1160}\u{FFA0}", InvalidCharacters),
            ("\u{2800}\u{2800}\u{2800}", InvalidCharacters),
            ("a\u{FE00}bc", InvalidCharacters),
            ("a\u{FE0D}bc", InvalidCharacters),
            ("\u{FE0F}abc", InvalidCharacters),
            ("ab \u{FE0F}c", InvalidCharacters),
            ("a\u{FE0F}\u{FE0F}bc", InvalidCharacters),
            ("abc\u{E0100}\u{E0101}\u{E01EF}", InvalidCharacters),
            ("a\u{180B}bc", InvalidCharacters),
            ("a\u{180F}bc", InvalidCharacters),
            ("abc\u{E0041}", InvalidCharacters),
            ("abc\u{E0067}\u{E007F}", InvalidCharacters),
            ("🏴\u{E007F}abc", InvalidCharacters),
            ("🏴\u{E0067}\u{E0062} abc", InvalidCharacters),
            ("abc 🏴\u{E0067}\u{E0062}", InvalidCharacters),
            ("abc\u{E0001}", InvalidCharacters),
            ("abc\u{0000}", InvalidCharacters),
            ("abc\u{0007}def", InvalidCharacters),
            ("abc\tdef", InvalidCharacters),
            ("abc\rdef", InvalidCharacters),
            ("abc\u{0085}def", InvalidCharacters),
            ("\u{0301}abc", InvalidCharacters),
            (zalgo.as_str(), InvalidCharacters),
            // Blank-looking: fewer than three visible characters.
            ("a\u{0301}\u{0301}", TooShort),
            ("a b", TooShort),
            ("a\u{3000}\u{2003}b", TooShort),
            // Visible characters are counted, not graphemes: a lone two-symbol flag is short.
            ("🏳\u{FE0F}\u{200D}🌈", TooShort),
        ] {
            assert_eq!(check_free_text(input), Err(expected), "{input:?}");
        }
    }

    #[test]
    fn length_counts_all_characters_and_bad_characters_win() {
        let long = "x".repeat(FREE_TEXT_MAX_CHARS + 1);
        assert_eq!(check_free_text(&long), Err(TooLong));
        let mut text = "x".repeat(FREE_TEXT_MAX_CHARS + 10);
        text.push('\u{202E}');
        assert_eq!(check_free_text(&text), Err(InvalidCharacters));
        // Spaces count towards the maximum.
        let spaced = format!("abc{}def", " ".repeat(FREE_TEXT_MAX_CHARS));
        assert_eq!(check_free_text(&spaced), Err(TooLong));
    }
}
