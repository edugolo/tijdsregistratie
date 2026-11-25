//! Free text typed by people: correction reasons and clock notes.

use unicode_normalization::char::canonical_combining_class;
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

pub const FREE_TEXT_MIN_CHARS: usize = 3;
pub const FREE_TEXT_MAX_CHARS: usize = 500;
/// The most combining marks (`Mn`/`Me`) allowed on one base character.
pub const FREE_TEXT_MAX_MARKS: usize = 4;

const ZWNJ: char = '\u{200C}';
const ZWJ: char = '\u{200D}';
const VS15: char = '\u{FE0E}';
const VS16: char = '\u{FE0F}';
const BLACK_FLAG: char = '\u{1F3F4}';
const TAG_DIGITS: std::ops::RangeInclusive<char> = '\u{E0030}'..='\u{E0039}';
const TAG_LOWERCASE: std::ops::RangeInclusive<char> = '\u{E0061}'..='\u{E007A}';
const CANCEL_TAG: char = '\u{E007F}';
/// Canonical combining class of a virama, which a ZWJ or ZWNJ may follow.
const VIRAMA_CLASS: u8 = 9;

/// Why a free text was refused, so a client can tell the person what to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeTextError {
    /// Fewer than [`FREE_TEXT_MIN_CHARS`] visible characters.
    TooShort,
    /// More than [`FREE_TEXT_MAX_CHARS`] characters in total.
    TooLong,
    /// An invisible, bidi or control character, or a misplaced joiner, selector, mark or tag.
    InvalidCharacters,
}

/// The trimmed text when it is acceptable, `None` otherwise. See [`check_free_text`].
pub fn free_text(value: &str) -> Option<&str> {
    check_free_text(value).ok()
}

/// Checks a correction reason or clock note and returns it trimmed.
///
/// Surrounding whitespace (spaces, tabs, CR/LF, line and paragraph separators) is trimmed
/// first. More than four bytes per allowed character is [`FreeTextError::TooLong`] before any
/// character is inspected. The rest is refused with [`FreeTextError::InvalidCharacters`] when
/// it holds:
/// - a control character other than `'\n'` (a `'\r'` is fine right before `'\n'`); a tab inside
///   the text is refused, not rewritten, so the stored text is exactly what was checked;
/// - a line or paragraph separator (`U+2028`, `U+2029`): `'\n'` is the only line break;
/// - a bidi control, zero-width space, word joiner or invisible operator, BOM, soft hyphen,
///   Mongolian vowel separator, combining grapheme joiner, Hangul filler or braille blank;
/// - an unassigned (`Cn`) or private-use (`Co`) code point;
/// - a ZWJ or ZWNJ that is not between two non-space characters, that is followed by a
///   combining mark, or that follows a combining mark other than a virama or VS15/VS16;
/// - a variation selector other than VS15/VS16 right after a non-space base character (so
///   `U+FE00`–`U+FE0D`, the supplement `U+E0100`–`U+E01EF` and the Mongolian free variation
///   selectors are always refused);
/// - a tag character outside a subdivision flag: `U+1F3F4`, two tag letters `a`–`z`, one to
///   four more tag letters or digits, then `U+E007F`;
/// - any other format character (`Cf`);
/// - a combining mark at the start or after whitespace, a control character, ZWJ or ZWNJ, or
///   more than [`FREE_TEXT_MAX_MARKS`] on one base character (joiners and variation selectors
///   between them do not start a new base).
///
/// Otherwise the text is [`FreeTextError::TooLong`] above [`FREE_TEXT_MAX_CHARS`] characters,
/// and [`FreeTextError::TooShort`] with fewer than [`FREE_TEXT_MIN_CHARS`] visible characters
/// (letters, numbers, punctuation and symbols).
pub fn check_free_text(value: &str) -> Result<&str, FreeTextError> {
    let value = value.trim();
    // A char is at most four bytes, so this bounds the work below without changing the result
    // for any text that could be short enough.
    if value.len() > 4 * FREE_TEXT_MAX_CHARS {
        return Err(FreeTextError::TooLong);
    }
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
    // Combining marks on the current base character.
    let mut marks = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        if c == BLACK_FLAG && next.is_some_and(is_tag) {
            let Some(len) = subdivision_tags(&chars[i + 1..]) else {
                return false;
            };
            i += 1 + len;
            marks = 0;
            continue;
        }
        let allowed = match c {
            '\n' => true,
            '\r' => next == Some('\n'),
            ZWJ | ZWNJ => {
                prev.is_some_and(|p| is_joinable(p) && (!is_combining(p) || joins_after_mark(p)))
                    && next.is_some_and(|n| is_joinable(n) && !is_combining(n))
            }
            VS15 | VS16 => prev.is_some_and(is_base),
            c => !is_refused(c),
        };
        if !allowed {
            return false;
        }
        if is_combining(c) {
            marks += 1;
            if !prev.is_some_and(can_carry_mark) || marks > FREE_TEXT_MAX_MARKS {
                return false;
            }
        } else if !matches!(c, ZWJ | ZWNJ | VS15 | VS16) {
            marks = 0;
        }
        i += 1;
    }
    true
}

/// The length of the tag run of a subdivision flag (letters, letters or digits, cancel tag) at
/// the start of `tags`, or `None` when the tags spell anything else.
fn subdivision_tags(tags: &[char]) -> Option<usize> {
    let end = tags.iter().position(|&c| c == CANCEL_TAG)?;
    let spec = &tags[..end];
    let valid = (3..=6).contains(&spec.len())
        && spec[..2].iter().all(|c| TAG_LOWERCASE.contains(c))
        && spec[2..]
            .iter()
            .all(|c| TAG_LOWERCASE.contains(c) || TAG_DIGITS.contains(c));
    valid.then_some(end + 1)
}

/// Characters that are refused wherever they stand (outside the exceptions in `chars_allowed`).
fn is_refused(c: char) -> bool {
    is_invisible(c)
        || matches!(
            c.general_category(),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Unassigned
                | GeneralCategory::PrivateUse
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
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

/// The combining marks (`Mn`) a ZWJ or ZWNJ may follow: a virama, as in Indic conjuncts, and
/// VS15/VS16, as in emoji ZWJ sequences.
fn joins_after_mark(c: char) -> bool {
    matches!(c, VS15 | VS16) || canonical_combining_class(c) == VIRAMA_CLASS
}

fn is_tag(c: char) -> bool {
    ('\u{E0000}'..=CANCEL_TAG).contains(&c)
}

/// A character a combining mark may follow: a base character, another mark or a selector.
fn can_carry_mark(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && c != ZWJ && c != ZWNJ
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

    /// The tag characters (`U+E0000` + ASCII) that spell `ascii`.
    fn tags(ascii: &str) -> String {
        ascii
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect()
    }

    /// A black flag followed by the tag spelling of `ascii` and a cancel tag.
    fn flag(ascii: &str) -> String {
        format!("🏴{}\u{E007F}", tags(ascii))
    }

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
            // Line and paragraph separators are only trimmed at the ends.
            ("\u{2029}abc\u{2028}", "abc"),
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
        for subdivision in ["gbsct", "gbwls", "gbeng", "usca", "be1234"] {
            let input = format!("{} op uitstap", flag(subdivision));
            assert_eq!(check_free_text(&input), Ok(input.as_str()), "{subdivision}");
        }
    }

    #[test]
    fn refused_texts_say_why() {
        let zalgo = format!("a{}bc", "\u{0300}".repeat(5));
        let zalgo_joined = format!(
            "a{}bc",
            "\u{0300}\u{0300}\u{0300}\u{0300}\u{200D}".repeat(100)
        );
        let zalgo_zwnj = format!(
            "a{}bc",
            "\u{0300}\u{0300}\u{0300}\u{0300}\u{200C}".repeat(3)
        );
        let zalgo_split = "a\u{0300}\u{0300}\u{0300}\u{200D}\u{0300}\u{0300}bc".to_string();
        let flag_payload = format!("{} abc", flag("hello world"));
        let flag_upper = format!("{} abc", flag("GBSCT"));
        let flag_short = format!("{} abc", flag("gb"));
        let flag_long = format!("{} abc", flag("gbabcde"));
        let flag_digit_first = format!("{} abc", flag("1bsct"));
        let flag_open = format!("🏴{} abc", tags("gbsct"));
        let wrong = [
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
            // Joiners do not start a new base: five marks on one base, split by a ZWJ or ZWNJ.
            (zalgo_joined.as_str(), InvalidCharacters),
            (zalgo_zwnj.as_str(), InvalidCharacters),
            (zalgo_split.as_str(), InvalidCharacters),
            ("ab\u{0301}\u{200D}c", InvalidCharacters),
            ("ab\u{200D}\u{0301}c", InvalidCharacters),
            ("ab\u{200C}\u{0301}c", InvalidCharacters),
            // A combining mark needs a base: not after a space, line break or joiner.
            ("ab \u{0301}c", InvalidCharacters),
            ("ab\n\u{0301}c", InvalidCharacters),
            ("ab\u{00A0}\u{0301}c", InvalidCharacters),
            ("ab\r\n\u{0301}c", InvalidCharacters),
            // Unassigned and private-use code points.
            ("abc\u{2065}def", InvalidCharacters),
            ("abc\u{FFF0}", InvalidCharacters),
            ("abc\u{FFF8}", InvalidCharacters),
            ("abc\u{E0000}", InvalidCharacters),
            ("abc\u{E0002}", InvalidCharacters),
            ("abc\u{E001F}", InvalidCharacters),
            ("abc\u{E000}", InvalidCharacters),
            ("abc\u{F8FF}", InvalidCharacters),
            ("abc\u{F0000}", InvalidCharacters),
            ("abc\u{10FFFD}", InvalidCharacters),
            // Tags only spell a subdivision flag.
            (flag_payload.as_str(), InvalidCharacters),
            (flag_upper.as_str(), InvalidCharacters),
            (flag_short.as_str(), InvalidCharacters),
            (flag_long.as_str(), InvalidCharacters),
            (flag_digit_first.as_str(), InvalidCharacters),
            (flag_open.as_str(), InvalidCharacters),
            // '\n' is the only line break inside the text.
            ("abc\u{2029}def", InvalidCharacters),
            ("abc\u{2028}def", InvalidCharacters),
            // Blank-looking: fewer than three visible characters.
            ("a\u{0301}\u{0301}", TooShort),
            ("a b", TooShort),
            ("a\u{3000}\u{2003}b", TooShort),
            // Visible characters are counted, not graphemes: a lone two-symbol flag is short.
            ("🏳\u{FE0F}\u{200D}🌈", TooShort),
        ]
        .into_iter()
        .filter(|&(input, expected)| check_free_text(input) != Err(expected))
        .map(|(input, expected)| (input, expected, check_free_text(input)))
        .collect::<Vec<_>>();
        assert!(wrong.is_empty(), "{wrong:#?}");
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
        // Oversized input is refused on its byte length, before any character is inspected.
        let mut huge = "x".repeat(4 * FREE_TEXT_MAX_CHARS + 1);
        huge.push('\u{202E}');
        assert_eq!(check_free_text(&huge), Err(TooLong));
        assert_eq!(check_free_text(&"\u{0300}".repeat(1_000_000)), Err(TooLong));
    }
}
