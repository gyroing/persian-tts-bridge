// persian_normalizer.rs
// Standalone Persian text normalizer (zero external dependencies).
// Full parity with persian_normalizer.cpp.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::OnceLock;

// ------------------------------------------------------------
// Helpers
// ------------------------------------------------------------

fn trim_ws(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n')
}

fn is_ascii_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

fn has_ascii_digit(s: &str) -> bool {
    s.bytes().any(|b| b.is_ascii_digit())
}

fn has_ascii_upper(s: &str) -> bool {
    s.bytes().any(|b| b.is_ascii_uppercase())
}

fn lower_ascii(s: &str) -> String {
    s.to_ascii_lowercase()
}

// ------------------------------------------------------------
// Emoji
// ------------------------------------------------------------

fn is_emoji(c: char) -> bool {
    let cp = c as u32;
    (0x1F600..=0x1F64F).contains(&cp)
        || (0x1F300..=0x1F5FF).contains(&cp)
        || (0x1F680..=0x1F6FF).contains(&cp)
        || (0x1F700..=0x1F77F).contains(&cp)
        || (0x1F780..=0x1F7FF).contains(&cp)
        || (0x1F800..=0x1F8FF).contains(&cp)
        || (0x1F900..=0x1F9FF).contains(&cp)
        || (0x1FA00..=0x1FAFF).contains(&cp)
        || (0x1F1E6..=0x1F1FF).contains(&cp)
        || (0x2600..=0x26FF).contains(&cp)
        || (0x2700..=0x27BF).contains(&cp)
        || (0x2B50..=0x2B55).contains(&cp)
}

// ------------------------------------------------------------
// Persian numbers
// ------------------------------------------------------------

const ONES: [&str; 10] = [
    "", "یک", "دو", "سه", "چهار", "پنج", "شش", "هفت", "هشت", "نه",
];

const TEENS: [&str; 10] = [
    "ده", "یازده", "دوازده", "سیزده", "چهارده",
    "پانزده", "شانزده", "هفده", "هجده", "نوزده",
];

const TENS: [&str; 10] = [
    "", "", "بیست", "سی", "چهل", "پنجاه", "شصت", "هفتاد", "هشتاد", "نود",
];

const HUNDREDS: [&str; 10] = [
    "", "صد", "دویست", "سیصد", "چهارصد",
    "پانصد", "ششصد", "هفتصد", "هشتصد", "نهصد",
];

fn under1000(mut n: usize) -> String {
    let mut p: Vec<&str> = Vec::with_capacity(3);

    if n >= 100 {
        p.push(HUNDREDS[n / 100]);
        n %= 100;
    }
    if n >= 20 {
        p.push(TENS[n / 10]);
        n %= 10;
    }
    if n >= 10 {
        p.push(TEENS[n - 10]);
        n = 0;
    }
    if n > 0 {
        p.push(ONES[n]);
    }

    p.join(" و ")
}

fn number_words(mut n: i64) -> String {
    if n == 0 {
        return "صفر".to_string();
    }

    const SCALES: [(i64, &str); 4] = [
        (1_000_000_000_000, "تریلیون"),
        (1_000_000_000, "میلیارد"),
        (1_000_000, "میلیون"),
        (1_000, "هزار"),
    ];

    let mut parts: Vec<String> = Vec::new();

    for &(scale, name) in &SCALES {
        if n >= scale {
            let count = n / scale;
            n %= scale;

            let head = if count == 1 && scale == 1000 {
                String::new()
            } else {
                format!("{} ", under1000(count as usize))
            };

            parts.push(format!("{}{}", head, name));
        }
    }

    if n > 0 {
        parts.push(under1000(n as usize));
    }

    parts.join(" و ")
}

fn digit_words(s: &str) -> String {
    let mut out = String::new();
    for (i, b) in s.bytes().enumerate() {
        let d = (b - b'0') as usize;
        if i > 0 {
            out.push(' ');
        }
        out.push_str(if d == 0 { "صفر" } else { ONES[d] });
    }
    out
}

// ------------------------------------------------------------
// Convert Persian and Arabic digits before date/time parsing
// ------------------------------------------------------------

fn normalize_digits(text: &str) -> String {
    const FA_DIGITS: [char; 10] = ['۰', '۱', '۲', '۳', '۴', '۵', '۶', '۷', '۸', '۹'];
    const AR_DIGITS: [char; 10] = ['٠', '١', '٢', '٣', '٤', '٥', '٦', '٧', '٨', '٩'];

    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if let Some(i) = FA_DIGITS.iter().position(|&d| d == c) {
            out.push((b'0' + i as u8) as char);
        } else if let Some(i) = AR_DIGITS.iter().position(|&d| d == c) {
            out.push((b'0' + i as u8) as char);
        } else {
            out.push(c);
        }
    }
    out
}

// ------------------------------------------------------------
// Exact regex-equivalent scanners for number, date, time, URL
// ------------------------------------------------------------

fn parse_hour_before_colon(b: &[u8], i: usize) -> Option<(usize, usize)> {
    // Matches ([01]?\d|2[0-3]) followed by ':'
    if i >= b.len() || !b[i].is_ascii_digit() {
        return None;
    }
    // Try 2-digit hour first (greedy)
    if i + 2 < b.len() && b[i + 1].is_ascii_digit() && b[i + 2] == b':' {
        let h0 = b[i];
        let h1 = b[i + 1];
        if (h0 == b'0' || h0 == b'1') || (h0 == b'2' && (b'0'..=b'3').contains(&h1)) {
            return Some((i, i + 2));
        }
    }
    // Try 1-digit hour
    if i + 1 < b.len() && b[i + 1] == b':' {
        return Some((i, i + 1));
    }
    None
}

fn is_two_digit_00_59(b: &[u8], i: usize) -> bool {
    i + 1 < b.len() && (b'0'..=b'5').contains(&b[i]) && b[i + 1].is_ascii_digit()
}

fn replace_dollar(text: &str) -> String {
    // \$\s*(\d[\d,.]*|\d) -> "$1 دلار "
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let mut j = i + 1;
            while j < b.len() {
                let ch = text[j..].chars().next().unwrap();
                if ch.is_whitespace() {
                    j += ch.len_utf8();
                } else {
                    break;
                }
            }
            if j < b.len() && b[j].is_ascii_digit() {
                let num_start = j;
                j += 1;
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == b',' || b[j] == b'.') {
                    j += 1;
                }
                out.push_str(&text[num_start..j]);
                out.push_str(" دلار ");
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_hms(text: &str) -> String {
    // ([01]?\d|2[0-3]):([0-5]\d):([0-5]\d) -> "$1 و $2 دقیقه و $3 ثانیه "
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if let Some((hs, he)) = parse_hour_before_colon(b, i) {
            let m_start = he + 1;
            if is_two_digit_00_59(b, m_start)
                && m_start + 2 < b.len()
                && b[m_start + 2] == b':'
                && is_two_digit_00_59(b, m_start + 3)
            {
                let s_start = m_start + 3;
                out.push_str(&text[hs..he]);
                out.push_str(" و ");
                out.push_str(&text[m_start..m_start + 2]);
                out.push_str(" دقیقه و ");
                out.push_str(&text[s_start..s_start + 2]);
                out.push_str(" ثانیه ");
                i = s_start + 2;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_h00(text: &str) -> String {
    // ([01]?\d|2[0-3]):00(?![\d:]) -> "$1 "
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if let Some((hs, he)) = parse_hour_before_colon(b, i) {
            let m_start = he + 1;
            if m_start + 1 < b.len() && b[m_start] == b'0' && b[m_start + 1] == b'0' {
                let after = m_start + 2;
                let ok_after = after == b.len() || (!b[after].is_ascii_digit() && b[after] != b':');
                if ok_after {
                    out.push_str(&text[hs..he]);
                    out.push(' ');
                    i = after;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_hm(text: &str) -> String {
    // ([01]?\d|2[0-3]):([0-5]\d) -> "$1 و $2 دقیقه "
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if let Some((hs, he)) = parse_hour_before_colon(b, i) {
            let m_start = he + 1;
            if is_two_digit_00_59(b, m_start) {
                out.push_str(&text[hs..he]);
                out.push_str(" و ");
                out.push_str(&text[m_start..m_start + 2]);
                out.push_str(" دقیقه ");
                i = m_start + 2;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn count_digits_at(b: &[u8], i: usize, max_n: usize) -> usize {
    let mut n = 0;
    while i + n < b.len() && n < max_n && b[i + n].is_ascii_digit() {
        n += 1;
    }
    n
}

fn replace_ymd(text: &str, sep: u8) -> String {
    // (\d{4})<sep>(\d{1,2})<sep>(\d{1,2}) -> "$1 $2 $3"
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if count_digits_at(b, i, 4) == 4 && i + 4 < b.len() && b[i + 4] == sep {
            let m_start = i + 5;
            let m_len = count_digits_at(b, m_start, 2);
            if m_len >= 1 && m_start + m_len < b.len() && b[m_start + m_len] == sep {
                let d_start = m_start + m_len + 1;
                let d_len = count_digits_at(b, d_start, 2);
                if d_len >= 1 {
                    out.push_str(&text[i..i + 4]);
                    out.push(' ');
                    out.push_str(&text[m_start..m_start + m_len]);
                    out.push(' ');
                    out.push_str(&text[d_start..d_start + d_len]);
                    i = d_start + d_len;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_dmy(text: &str, sep: u8) -> String {
    // (\d{1,2})<sep>(\d{1,2})<sep>(\d{4}) -> "$1 $2 $3"
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let d_len = count_digits_at(b, i, 2);
        if d_len >= 1 && i + d_len < b.len() && b[i + d_len] == sep {
            let m_start = i + d_len + 1;
            let m_len = count_digits_at(b, m_start, 2);
            if m_len >= 1 && m_start + m_len < b.len() && b[m_start + m_len] == sep {
                let y_start = m_start + m_len + 1;
                if count_digits_at(b, y_start, 4) == 4 {
                    out.push_str(&text[i..i + d_len]);
                    out.push(' ');
                    out.push_str(&text[m_start..m_start + m_len]);
                    out.push(' ');
                    out.push_str(&text[y_start..y_start + 4]);
                    i = y_start + 4;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_decimal_slash(text: &str) -> String {
    // (^|[^0-9/])([0-9]+)/([0-9]+)(?![0-9/]) -> "$1$2.$3"
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let prev_ok = i == 0 || (!b[i - 1].is_ascii_digit() && b[i - 1] != b'/');
        if prev_ok && b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && b[j] == b'/' && j + 1 < b.len() && b[j + 1].is_ascii_digit() {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                let next_ok = k == b.len() || (!b[k].is_ascii_digit() && b[k] != b'/');
                if next_ok {
                    out.push_str(&text[i..j]);
                    out.push('.');
                    out.push_str(&text[j + 1..k]);
                    i = k;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_thousands(text: &str) -> String {
    // (\d),(?=\d{3}(?:\D|$)) -> "$1"
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b','
            && i > 0
            && b[i - 1].is_ascii_digit()
            && i + 3 < b.len()
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
            && b[i + 3].is_ascii_digit()
            && (i + 4 == b.len() || !b[i + 4].is_ascii_digit())
        {
            i += 1;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn replace_negative(text: &str) -> String {
    // (^|[\s(=:])(?:-|−)(\d) -> "$1 منفی $2"
    let mut out = String::with_capacity(text.len() + 16);
    let mut iter = text.char_indices().peekable();
    let mut prev_ok = true;

    while let Some((idx, ch)) = iter.next() {
        if prev_ok && (ch == '-' || ch == '−') {
            if let Some(&(_, next_ch)) = iter.peek() {
                if next_ch.is_ascii_digit() {
                    out.push_str(" منفی ");
                    prev_ok = false;
                    continue;
                }
            }
        }
        out.push(ch);
        prev_ok = ch.is_whitespace() || ch == '(' || ch == '=' || ch == ':';
        let _ = idx;
    }
    out
}

fn numbers_to_words(text: &str) -> String {
    let mut s = text.replace('٫', ".").replace('٬', ",");
    s = normalize_digits(&s);

    if !has_ascii_digit(&s) {
        return s;
    }

    s = replace_dollar(&s);
    s = replace_hms(&s);
    s = replace_h00(&s);
    s = replace_hm(&s);
    s = replace_ymd(&s, b'/');
    s = replace_ymd(&s, b'-');
    s = replace_dmy(&s, b'/');
    s = replace_dmy(&s, b'-');
    s = replace_decimal_slash(&s);
    s = s.replace('/', " ");
    s = replace_thousands(&s);
    s = replace_negative(&s);

    let b = s.as_bytes();
    let mut result = String::with_capacity(s.len() * 2);
    let mut i = 0;

    while i < b.len() {
        if b[i].is_ascii_digit() {
            let w_start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let whole = &s[w_start..i];

            let frac = if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                let f_start = i + 1;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                &s[f_start..i]
            } else {
                ""
            };

            let w = whole.trim_start_matches('0');
            let w = if w.is_empty() { "0" } else { w };

            let mut spoken;
            if whole.len() > 12 || (whole.len() > 2 && whole.starts_with('0')) {
                spoken = digit_words(whole);
                if !frac.is_empty() {
                    spoken.push(' ');
                    spoken.push_str(&digit_words(frac));
                }
            } else {
                spoken = number_words(w.parse::<i64>().unwrap_or(0));
                if !frac.is_empty() {
                    let f = frac.trim_end_matches('0');
                    if !f.is_empty() {
                        let unit = match f.len() {
                            1 => "دهم",
                            2 => "صدم",
                            3 => "هزارم",
                            _ => "",
                        };
                        spoken.push_str(" ممیز ");
                        if !unit.is_empty() {
                            spoken.push_str(&number_words(f.parse::<i64>().unwrap_or(0)));
                            spoken.push(' ');
                            spoken.push_str(unit);
                        } else {
                            spoken.push_str(&digit_words(f));
                        }
                    }
                }
            }

            result.push(' ');
            result.push_str(&spoken);
            result.push(' ');
        } else {
            let ch = s[i..].chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }

    result
}

// ------------------------------------------------------------
// Persian normalization
// ------------------------------------------------------------

fn strip_urls(text: &str) -> String {
    if !text.contains("http") && !text.contains("www.") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("www.") {
            let mut j = i;
            while j < text.len() {
                let ch = text[j..].chars().next().unwrap();
                if ch.is_whitespace() {
                    break;
                }
                j += ch.len_utf8();
            }
            i = j;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn clean_persian(input: &str) -> String {
    let mut text = strip_urls(input);

    let filtered: String = text
        .chars()
        .filter(|&c| c == '\u{200C}' || !is_emoji(c))
        .collect();

    text = numbers_to_words(&filtered);

    const REPLACEMENTS: [(&str, &str); 54] = [
        ("%", " درصد "),
        ("٪", " درصد "),
        ("\u{200E}", ""),
        ("\u{200F}", ""),
        ("\u{202A}", ""),
        ("\u{202B}", ""),
        ("\u{202C}", ""),
        ("ـ", ""),
        ("ء", ""),
        ("َ", ""),
        ("ِ", ""),
        ("ُ", ""),
        ("ً", ""),
        ("ٌ", ""),
        ("ٍ", ""),
        ("ّ", ""),
        ("ْ", ""),
        ("ي", "ی"),
        ("ى", "ی"),
        ("ۍ", "ی"),
        ("ې", "ی"),
        ("ك", "ک"),
        ("ڪ", "ک"),
        ("ة", "ه"),
        ("ۀ", "ه"),
        ("ۂ", "ه"),
        ("ۃ", "ه"),
        ("أ", "ا"),
        ("إ", "ا"),
        ("ٱ", "ا"),
        ("ٲ", "ا"),
        ("ٳ", "ا"),
        ("ؤ", "و"),
        (",", "،"),
        ("?", "؟"),
        (";", "؛"),
        ("…", "."),
        ("–", " "),
        ("—", " "),
        ("−", " "),
        ("-", " "),
        ("/", " "),
        ("\\", " "),
        ("«", " "),
        ("»", " "),
        ("“", " "),
        ("”", " "),
        ("‘", " "),
        ("\"", " "),
        ("(", " "),
        (")", " "),
        ("[", " "),
        ("]", " "),
        ("{", " "),
    ];

    for &(a, b) in &REPLACEMENTS {
        if text.contains(a) {
            text = text.replace(a, b);
        }
    }
    for &(a, b) in &[("}", " "), ("\n", " "), ("\r", " "), ("\t", " ")] {
        if text.contains(a) {
            text = text.replace(a, b);
        }
    }

    text
}

// ------------------------------------------------------------
// Latin accent folding
// ------------------------------------------------------------

fn fold_latin(s: &str) -> String {
    const L1: [Option<&str>; 64] = [
        Some("A"), Some("A"), Some("A"), Some("A"), Some("A"), Some("A"), Some("AE"), Some("C"),
        Some("E"), Some("E"), Some("E"), Some("E"), Some("I"), Some("I"), Some("I"), Some("I"),
        Some("D"), Some("N"), Some("O"), Some("O"), Some("O"), Some("O"), Some("O"), None,
        Some("O"), Some("U"), Some("U"), Some("U"), Some("U"), Some("Y"), Some("TH"), Some("ss"),
        Some("a"), Some("a"), Some("a"), Some("a"), Some("a"), Some("a"), Some("ae"), Some("c"),
        Some("e"), Some("e"), Some("e"), Some("e"), Some("i"), Some("i"), Some("i"), Some("i"),
        Some("d"), Some("n"), Some("o"), Some("o"), Some("o"), Some("o"), Some("o"), None,
        Some("o"), Some("u"), Some("u"), Some("u"), Some("u"), Some("y"), Some("th"), Some("y"),
    ];

    if !s.as_bytes().contains(&0xC3) {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if (0xC0..=0xFF).contains(&cp) {
            if let Some(rep) = L1[(cp - 0xC0) as usize] {
                out.push_str(rep);
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------
// Options
//   G2P_TH=stop : English TH/DH -> ت / د (default: fricative س / ز)
// ------------------------------------------------------------

fn th_stop() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| env::var("G2P_TH").map(|v| v == "stop").unwrap_or(false))
}

// ------------------------------------------------------------
// Latin spelling: CMUdict (ARPAbet) -> Persian letters
// ------------------------------------------------------------

struct ArpaPhone<'a> {
    base: &'a str,
    stress: i32,
    vowel: bool,
}

fn arpa_is_vowel(p: &str) -> bool {
    matches!(
        p,
        "AA" | "AE" | "AH" | "AO" | "AW" | "AY"
            | "EH" | "ER" | "EY" | "IH" | "IY"
            | "OW" | "OY" | "UH" | "UW"
    )
}

fn arpa_consonant(p: &str) -> Option<&'static str> {
    match p {
        "B" => Some("ب"),
        "CH" => Some("چ"),
        "D" => Some("د"),
        "F" => Some("ف"),
        "G" => Some("گ"),
        "HH" => Some("ه"),
        "JH" => Some("ج"),
        "K" => Some("ک"),
        "L" => Some("ل"),
        "M" => Some("م"),
        "N" => Some("ن"),
        "P" => Some("پ"),
        "R" => Some("ر"),
        "S" => Some("س"),
        "SH" => Some("ش"),
        "T" => Some("ت"),
        "V" => Some("و"),
        "W" => Some("و"),
        "Y" => Some("ی"),
        "Z" => Some("ز"),
        "ZH" => Some("ژ"),
        _ => None,
    }
}

fn parse_arpa(ph: &[String]) -> Vec<ArpaPhone<'_>> {
    let mut out = Vec::with_capacity(ph.len());
    for raw in ph {
        if raw.is_empty() {
            continue;
        }
        let bytes = raw.as_bytes();
        let last = bytes[bytes.len() - 1];
        let (base, stress) = if (b'0'..=b'2').contains(&last) {
            (&raw[..raw.len() - 1], (last - b'0') as i32)
        } else {
            (raw.as_str(), -1)
        };
        let vowel = arpa_is_vowel(base);
        out.push(ArpaPhone {
            base,
            stress: if vowel { stress } else { -1 },
            vowel,
        });
    }
    out
}

fn arpa_to_persian(ph: &[String]) -> String {
    let p_list = parse_arpa(ph);
    if p_list.is_empty() {
        return String::new();
    }
    let n = p_list.len();

    let mut first_nuc: isize = -1;
    let mut first_stressed: isize = -1;

    for (i, phone) in p_list.iter().enumerate() {
        if !phone.vowel {
            continue;
        }
        if first_nuc < 0 {
            first_nuc = i as isize;
        }
        if first_stressed < 0 && (phone.stress == 1 || phone.stress == 2) {
            first_stressed = i as isize;
        }
    }

    let emits_vowel_letter = |j: usize| -> bool {
        if j >= n || !p_list[j].vowel {
            return false;
        }
        let b = p_list[j].base;
        if b == "ER" {
            return j == 0;
        }
        if b == "AE" || b == "EH" {
            return j == 0 || j + 1 == n || p_list[j - 1].vowel;
        }
        if b == "AH" {
            let st = p_list[j].stress == 1 || p_list[j].stress == 2;
            let pre = (j as isize) == first_nuc && j > 0 && first_stressed > (j as isize);
            let prev_v = j > 0 && p_list[j - 1].vowel;
            let ial_ual = prev_v
                && j + 1 < n
                && p_list[j + 1].base == "L"
                && (p_list[j - 1].base == "IY" || p_list[j - 1].base == "UW");
            if j == 0 || j + 1 == n || st || ial_ual {
                return true;
            }
            if prev_v {
                return false;
            }
            return pre;
        }
        true
    };

    let glide_follows = |i: usize| -> bool {
        if i + 1 >= n {
            return false;
        }
        let nx = p_list[i + 1].base;
        if nx != "V" && nx != "W" {
            return false;
        }
        !emits_vowel_letter(i + 2)
    };

    let initial_s_cluster = n >= 2
        && p_list[0].base == "S"
        && !p_list[1].vowel
        && p_list[1].base != "W"
        && p_list[1].base != "Y";

    let mut out = String::with_capacity(n * 3 + 3);
    if initial_s_cluster {
        out.push('ا');
    }

    for i in 0..n {
        let phone = &p_list[i];
        let p = phone.base;
        let at_start = i == 0;
        let at_end = i + 1 == n;
        let prev_vowel = i > 0 && p_list[i - 1].vowel;
        let prev = if i > 0 { p_list[i - 1].base } else { "" };
        let next = if at_end { "" } else { p_list[i + 1].base };

        if !phone.vowel {
            if i > 0 && !p_list[i - 1].vowel && prev == p {
                continue;
            }
            if p == "NG" {
                out.push_str(if next == "K" || next == "G" { "ن" } else { "نگ" });
            } else if p == "TH" {
                out.push_str(if th_stop() { "ت" } else { "س" });
            } else if p == "DH" {
                out.push_str(if th_stop() { "د" } else { "ز" });
            } else if let Some(c) = arpa_consonant(p) {
                out.push_str(c);
            }
            continue;
        }

        let st = phone.stress;
        let stressed = st == 1 || st == 2;
        let pretonic_first =
            (i as isize) == first_nuc && i > 0 && st <= 0 && first_stressed > (i as isize);
        let after_c_w = i >= 2 && !p_list[i - 2].vowel && prev == "W";

        let omitted = if glide_follows(i) {
            "ا"
        } else if after_c_w {
            if p == "AE" { "ا" } else { "ئ" }
        } else {
            ""
        };

        let v: &str = match p {
            "AA" => if at_start { "آ" } else { "ا" },
            "AE" => {
                if at_start || prev_vowel { "ا" }
                else if at_end { "ه" }
                else { omitted }
            }
            "AH" => {
                if at_start {
                    if stressed { "آ" } else { "ا" }
                } else if at_end {
                    "ا"
                } else if prev_vowel {
                    if stressed {
                        "ا"
                    } else if next == "L" && (prev == "IY" || prev == "UW") {
                        "ا"
                    } else {
                        ""
                    }
                } else if stressed || pretonic_first {
                    "ا"
                } else {
                    omitted
                }
            }
            "AO" => {
                if next == "R" {
                    if at_start { "او" } else { "و" }
                } else if at_start {
                    "آ"
                } else {
                    "ا"
                }
            }
            "AW" => if at_start { "آو" } else { "او" },
            "AY" => if at_start { "آی" } else { "ای" },
            "EH" => {
                if at_start || prev_vowel { "ا" }
                else if at_end { "ه" }
                else { omitted }
            }
            "ER" => {
                if at_start {
                    "ار"
                } else if st <= 0 && prev_vowel && (prev == "OW" || prev == "UW") {
                    "ئر"
                } else {
                    "ر"
                }
            }
            "EY" => if at_start || prev_vowel { "ای" } else { "ی" },
            "IH" | "IY" => if at_start { "ای" } else { "ی" },
            "OW" => if at_start { "او" } else { "و" },
            "OY" => if at_start { "اوی" } else { "وی" },
            "UH" | "UW" => if at_start { "او" } else { "و" },
            _ => "",
        };

        out.push_str(v);
    }

    out
}

// ------------------------------------------------------------
// English letter helpers & morphological analysis for OOV words
// ------------------------------------------------------------

fn en_vowel(b: u8) -> bool {
    matches!(b, b'a' | b'e' | b'i' | b'o' | b'u' | b'y')
}

fn has_en_vowel(s: &str) -> bool {
    s.bytes().any(en_vowel)
}

fn ends_single_vc(b: &str) -> bool {
    let bytes = b.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let last = bytes[bytes.len() - 1];
    let prev = bytes[bytes.len() - 2];
    if en_vowel(last) || matches!(last, b'w' | b'x' | b'y') {
        return false;
    }
    if !en_vowel(prev) {
        return false;
    }
    if bytes.len() >= 3 && en_vowel(bytes[bytes.len() - 3]) {
        return false;
    }
    true
}

#[derive(Clone, Copy)]
enum MorphKind {
    S,
    Ed,
    Ing,
    Er,
    Est,
    Ly,
    Less,
    Ful,
    Ness,
    Ment,
    Able,
}

struct Morph {
    base: String,
    kind: MorphKind,
}

fn morph_bases(n: &str) -> Vec<Morph> {
    let mut v = Vec::new();
    let l = n.len();

    fn add(v: &mut Vec<Morph>, b: String, kind: MorphKind) {
        if b.len() >= 3 {
            v.push(Morph { base: b, kind });
        }
    }

    fn add_stem_variants(v: &mut Vec<Morph>, b: &str, kind: MorphKind) {
        let bytes = b.as_bytes();
        let doubled = bytes.len() >= 4
            && !en_vowel(bytes[bytes.len() - 1])
            && bytes[bytes.len() - 1] == bytes[bytes.len() - 2];

        if doubled {
            add(v, b.to_string(), kind);
            add(v, format!("{}e", b), kind);
            add(v, b[..b.len() - 1].to_string(), kind);
        } else if ends_single_vc(b) {
            add(v, format!("{}e", b), kind);
            add(v, b.to_string(), kind);
        } else {
            add(v, b.to_string(), kind);
            add(v, format!("{}e", b), kind);
        }
    }

    if n.ends_with("ies") {
        add(&mut v, format!("{}y", &n[..l - 3]), MorphKind::S);
    }
    if n.ends_with('s') && !n.ends_with("ss") {
        add(&mut v, n[..l - 1].to_string(), MorphKind::S);
    }
    if n.ends_with("es") {
        add(&mut v, n[..l - 2].to_string(), MorphKind::S);
    }

    if n.ends_with("ied") {
        add(&mut v, format!("{}y", &n[..l - 3]), MorphKind::Ed);
    }
    if n.ends_with("ed") && l >= 4 {
        add_stem_variants(&mut v, &n[..l - 2], MorphKind::Ed);
    }

    if n.ends_with("ing") && l >= 5 {
        add_stem_variants(&mut v, &n[..l - 3], MorphKind::Ing);
    }

    if n.ends_with("ier") {
        add(&mut v, format!("{}y", &n[..l - 3]), MorphKind::Er);
    }
    if n.ends_with("er") && l >= 4 {
        add_stem_variants(&mut v, &n[..l - 2], MorphKind::Er);
    }

    if n.ends_with("iest") {
        add(&mut v, format!("{}y", &n[..l - 4]), MorphKind::Est);
    }
    if n.ends_with("est") && l >= 5 {
        add_stem_variants(&mut v, &n[..l - 3], MorphKind::Est);
    }

    if n.ends_with("ily") {
        add(&mut v, format!("{}y", &n[..l - 3]), MorphKind::Ly);
    }
    if n.ends_with("ly") && l >= 5 {
        add(&mut v, n[..l - 2].to_string(), MorphKind::Ly);
    }

    if n.ends_with("less") && l >= 7 {
        add(&mut v, n[..l - 4].to_string(), MorphKind::Less);
    }
    if n.ends_with("ful") && l >= 6 {
        add(&mut v, n[..l - 3].to_string(), MorphKind::Ful);
    }
    if n.ends_with("iness") && l >= 8 {
        add(&mut v, format!("{}y", &n[..l - 5]), MorphKind::Ness);
    }
    if n.ends_with("ness") && l >= 7 {
        add(&mut v, n[..l - 4].to_string(), MorphKind::Ness);
    }
    if n.ends_with("ment") && l >= 7 {
        add(&mut v, n[..l - 4].to_string(), MorphKind::Ment);
    }
    if n.ends_with("able") && l >= 7 {
        add_stem_variants(&mut v, &n[..l - 4], MorphKind::Able);
    }

    v
}

fn morph_suffix(kind: MorphKind, last_raw: &str) -> &'static str {
    let last = last_raw.trim_end_matches(|c: char| c.is_ascii_digit());
    match kind {
        MorphKind::S => {
            if matches!(last, "S" | "Z" | "SH" | "ZH" | "CH" | "JH") {
                "یز"
            } else if matches!(last, "P" | "T" | "K" | "F" | "TH") {
                "س"
            } else {
                "ز"
            }
        }
        MorphKind::Ed => {
            if matches!(last, "T" | "D") {
                "ید"
            } else if matches!(last, "P" | "K" | "F" | "S" | "SH" | "CH" | "TH") {
                "ت"
            } else {
                "د"
            }
        }
        MorphKind::Ing => "ینگ",
        MorphKind::Er => "ر",
        MorphKind::Est => "ست",
        MorphKind::Ly => "لی",
        MorphKind::Less => "لس",
        MorphKind::Ful => "فول",
        MorphKind::Ness => "نس",
        MorphKind::Ment => "منت",
        MorphKind::Able => "ابل",
    }
}

// ------------------------------------------------------------
// Rule-based fallback for unknown Latin words
// ------------------------------------------------------------

fn rule_single(c: u8) -> Option<&'static str> {
    match c {
        b'b' => Some("ب"),
        b'd' => Some("د"),
        b'f' => Some("ف"),
        b'h' => Some("ه"),
        b'j' => Some("ج"),
        b'k' => Some("ک"),
        b'l' => Some("ل"),
        b'm' => Some("م"),
        b'n' => Some("ن"),
        b'p' => Some("پ"),
        b'q' => Some("ک"),
        b'r' => Some("ر"),
        b's' => Some("س"),
        b't' => Some("ت"),
        b'v' => Some("و"),
        b'w' => Some("و"),
        b'x' => Some("کس"),
        b'z' => Some("ز"),
        _ => None,
    }
}

fn rule_core(mut s: Vec<u8>) -> String {
    const MULTI: [(&[u8], &str); 28] = [
        (b"eigh", "ای"), (b"tch", "چ"), (b"dge", "ج"), (b"igh", "ای"), (b"sch", "ش"),
        (b"sh", "ش"), (b"ch", "چ"), (b"ph", "ف"), (b"kh", "خ"),
        (b"gh", "گ"), (b"ck", "ک"), (b"qu", "کو"), (b"ng", "نگ"),
        (b"oo", "و"), (b"ee", "ی"), (b"ea", "ی"), (b"ie", "ی"),
        (b"ai", "ی"), (b"ay", "ی"), (b"ou", "او"), (b"ow", "او"),
        (b"oa", "و"), (b"ew", "یو"), (b"ui", "ی"), (b"au", "ا"),
        (b"aw", "ا"), (b"oi", "وی"), (b"oy", "وی"),
    ];

    if s.starts_with(b"kn") || s.starts_with(b"gn") || s.starts_with(b"ps") || s.starts_with(b"wr") {
        s.remove(0);
    } else if s.starts_with(b"wh") {
        s.remove(1);
    }

    if s.len() > 3 && s.ends_with(b"que") {
        let new_len = s.len() - 3;
        s.truncate(new_len);
        s.push(b'k');
    }

    let len = s.len();
    if len >= 3
        && s[len - 1] == b'e'
        && !en_vowel(s[len - 2])
        && en_vowel(s[len - 3])
        && s[len - 3] != b'e'
        && !(len >= 4 && en_vowel(s[len - 4]))
    {
        s[len - 3] -= 32;
        s.pop();
        let last = s.len() - 1;
        if s[last] == b'c' {
            s[last] = b's';
        } else if s[last] == b'g' {
            s[last] = b'j';
        }
    } else if len > 2 && s[len - 1] == b'e' && !en_vowel(s[len - 2]) {
        s.pop();
    }

    let mut prothesis = false;
    if s.len() >= 2
        && s[0] == b's'
        && !en_vowel(s[1])
        && !matches!(s[1], b'h' | b'w' | b'y')
    {
        let soft_sc = s[1] == b'c' && s.len() >= 3 && matches!(s[2], b'h' | b'e' | b'i' | b'y');
        if !soft_sc {
            prothesis = true;
        }
    }

    let mut out = String::new();
    if prothesis {
        out.push('ا');
    }

    let mut i = 0;
    while i < s.len() {
        if s[i] == b's'
            && i + 2 < s.len()
            && s[i + 1] == b'c'
            && matches!(s[i + 2], b'e' | b'i' | b'y')
        {
            out.push('س');
            i += 2;
            continue;
        }

        let mut matched = false;
        for &(pat, fa) in &MULTI {
            if s[i..].starts_with(pat) {
                out.push_str(fa);
                i += pat.len();
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        let c = s[i];
        let first = i == 0;
        let next = if i + 1 < s.len() { s[i + 1] } else { 0 };

        if i > 0 && c == s[i - 1] && !en_vowel(c) {
            i += 1;
            continue;
        }

        match c {
            b'A' => out.push_str(if first { "ای" } else { "ی" }),
            b'I' | b'Y' => out.push_str(if first { "آی" } else { "ای" }),
            b'O' => out.push_str(if first { "او" } else { "و" }),
            b'U' => {
                let prev = if i > 0 { s[i - 1] } else { 0 };
                out.push_str(if matches!(prev, b'r' | b'l' | b'j') { "و" } else { "یو" });
            }
            b'a' => out.push('ا'),
            b'e' => {
                if first {
                    out.push('ا');
                }
            }
            b'i' => out.push_str(if first { "ای" } else { "ی" }),
            b'o' | b'u' => out.push_str(if first { "او" } else { "و" }),
            b'y' => out.push('ی'),
            b'c' => out.push_str(if matches!(next, b'e' | b'i' | b'y') { "س" } else { "ک" }),
            b'g' => out.push_str(if matches!(next, b'e' | b'i' | b'y') { "ج" } else { "گ" }),
            b't' if next == b'h' => {
                out.push_str(if th_stop() { "ت" } else { "س" });
                i += 1;
            }
            _ => {
                if let Some(ch) = rule_single(c) {
                    out.push_str(ch);
                }
            }
        }

        i += 1;
    }

    out
}

fn rule_based(word: &str) -> String {
    let mut s = lower_ascii(word);

    const SUF: [(&str, &str); 21] = [
        ("tion", "شن"), ("sion", "ژن"), ("ture", "چر"), ("able", "ابل"),
        ("ible", "یبل"), ("ment", "منت"), ("ness", "نس"), ("less", "لس"),
        ("ful", "فول"), ("ity", "یتی"), ("ous", "اس"), ("ize", "ایز"),
        ("ise", "ایز"), ("ish", "یش"), ("ist", "یست"), ("ism", "یزم"),
        ("ive", "یو"), ("ing", "ینگ"), ("ers", "رز"), ("er", "ر"),
        ("ly", "لی"),
    ];

    let mut tail = String::new();

    if s.len() > 4 && s.ends_with("ed") && !en_vowel(s.as_bytes()[s.len() - 3]) {
        let bytes = s.as_bytes();
        let c = bytes[bytes.len() - 3];
        let digraph_voiceless =
            c == b'h' && bytes.len() > 5 && matches!(bytes[bytes.len() - 4], b'c' | b's' | b't');
        let mut stem = s[..s.len() - 2].to_string();
        if ends_single_vc(&stem) {
            stem.push('e');
        }
        s = stem;

        tail = if matches!(c, b't' | b'd') {
            "ید".to_string()
        } else if digraph_voiceless || matches!(c, b'p' | b'k' | b'f' | b's' | b'x' | b'c') {
            "ت".to_string()
        } else {
            "د".to_string()
        };
    } else if s.len() > 3
        && s.ends_with('s')
        && !s.ends_with("ss")
        && !s.ends_with("us")
        && !s.ends_with("is")
    {
        let bytes = s.as_bytes();
        let es = s.ends_with("es")
            && s.len() > 4
            && (matches!(bytes[bytes.len() - 3], b's' | b'x' | b'z')
                || s.ends_with("shes")
                || s.ends_with("ches"));
        let stem = s[..s.len() - if es { 2 } else { 1 }].to_string();
        let sb = stem.as_bytes();
        let c = if sb.len() >= 2 && sb[sb.len() - 1] == b'e' {
            sb[sb.len() - 2]
        } else {
            sb[sb.len() - 1]
        };
        let sib = matches!(c, b's' | b'x' | b'z')
            || stem.ends_with("sh")
            || stem.ends_with("ch")
            || stem.ends_with("ce")
            || stem.ends_with("ge");

        tail = if sib {
            "یز".to_string()
        } else if matches!(c, b'p' | b't' | b'k' | b'f') {
            "س".to_string()
        } else {
            "ز".to_string()
        };
        s = stem;
    }

    for &(en, fa0) in &SUF {
        if s.len() >= en.len() + 2 && s.ends_with(en) {
            let stem = s[..s.len() - en.len()].to_string();
            let fa_suf = if en == "sion" {
                if en_vowel(stem.as_bytes()[stem.len() - 1]) { "ژن" } else { "شن" }
            } else {
                fa0
            };
            tail = format!("{}{}", fa_suf, tail);
            s = stem;
            break;
        }
    }

    format!("{}{}", rule_core(s.into_bytes()), tail)
}

// ------------------------------------------------------------
// CamelCase splitting
// ------------------------------------------------------------

fn split_camel(token: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let b = token.as_bytes();
    if b.len() < 2 {
        return parts;
    }

    let mut start = 0;
    for i in 1..b.len() {
        let prev_lo = b[i - 1].is_ascii_lowercase();
        let curr_up = b[i].is_ascii_uppercase();
        let prev_up = b[i - 1].is_ascii_uppercase();
        let next_lo = i + 1 < b.len() && b[i + 1].is_ascii_lowercase();

        if (prev_lo && curr_up) || (prev_up && curr_up && next_lo) {
            parts.push(&token[start..i]);
            start = i;
        }
    }

    if start > 0 {
        parts.push(&token[start..]);
    }
    parts
}

// ------------------------------------------------------------
// Latin dictionary (exact + case-insensitive) and CMUdict
// ------------------------------------------------------------

fn strip_apostrophe(s: &str) -> String {
    s.replace('\'', "")
}

struct UserDict {
    exact: HashMap<String, String>,
    lower_map: HashMap<String, String>,
}

fn load_user_dict(path: &str) -> UserDict {
    let mut dict = UserDict {
        exact: HashMap::new(),
        lower_map: HashMap::new(),
    };
    let Ok(file) = File::open(path) else {
        return dict;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = trim_ws(&line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(p) = line.find(|c| c == ' ' || c == '\t') else {
            continue;
        };
        let key = line[..p].to_string();
        let val = trim_ws(&line[p..]).to_string();
        if has_ascii_upper(&key) {
            dict.exact.insert(key, val);
        } else {
            dict.lower_map.insert(key, val);
        }
    }
    dict
}

fn find_user<'a>(user: &'a UserDict, token: &str) -> Option<&'a str> {
    if let Some(v) = user.exact.get(token) {
        return Some(v.as_str());
    }
    let tok_no_ap = strip_apostrophe(token);
    if tok_no_ap != token {
        if let Some(v) = user.exact.get(&tok_no_ap) {
            return Some(v.as_str());
        }
    }
    let w = lower_ascii(token);
    if let Some(v) = user.lower_map.get(&w) {
        return Some(v.as_str());
    }
    let n = strip_apostrophe(&w);
    if n != w {
        if let Some(v) = user.lower_map.get(&n) {
            return Some(v.as_str());
        }
    }
    None
}

fn load_cmu(path: &str, needed: &HashSet<String>) -> HashMap<String, Vec<String>> {
    let mut dict = HashMap::new();
    if needed.is_empty() {
        return dict;
    }

    let mut first_letter = [false; 256];
    let mut len_mask: u64 = 0;
    let mut has_long = false;

    for w in needed {
        if w.is_empty() {
            continue;
        }
        first_letter[w.as_bytes()[0] as usize] = true;
        if w.len() < 64 {
            len_mask |= 1u64 << w.len();
        } else {
            has_long = true;
        }
    }

    let Ok(file) = File::open(path) else {
        return dict;
    };

    for raw_line in BufReader::new(file).lines().map_while(Result::ok) {
        let bytes = raw_line.as_bytes();
        if bytes.len() < 3 || bytes[0] == b';' || bytes[0] == b'#' {
            continue;
        }
        let c0 = bytes[0].to_ascii_lowercase();
        if !first_letter[c0 as usize] {
            continue;
        }
        let line = raw_line.trim_end_matches('\r');
        let Some(p) = line.find(|c| c == ' ' || c == '\t') else {
            continue;
        };
        if p < 64 {
            if (len_mask & (1u64 << p)) == 0 {
                continue;
            }
        } else if !has_long {
            continue;
        }

        let word = lower_ascii(&line[..p]);
        if word.contains('(') || !needed.contains(&word) || dict.contains_key(&word) {
            continue;
        }

        let mut phones = Vec::new();
        for ph in line[p..].split_whitespace() {
            if ph.starts_with('#') {
                break;
            }
            phones.push(ph.to_string());
        }
        if !phones.is_empty() {
            dict.insert(word, phones);
        }
    }

    dict
}

fn spell_letters(word: &str) -> String {
    const NAMES: [&str; 26] = [
        "ای", "بی", "سی", "دی", "ای", "اف", "جی", "اچ", "آی",
        "جی", "کی", "ال", "ام", "ان", "او", "پی", "کیو", "آر",
        "اس", "تی", "یو", "وی", "دابلیو", "اکس", "وای", "زد",
    ];
    let mut out = String::new();
    for b in word.bytes() {
        if !is_ascii_alpha(b) {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(NAMES[((b | 32) - b'a') as usize]);
    }
    out
}

fn join_parts(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    let spaced = a.contains(' ') || b.contains(' ');
    let mut out = String::with_capacity(a.len() + b.len() + 3);
    out.push_str(a);
    if spaced {
        out.push(' ');
    } else {
        out.push('\u{200C}');
    }
    out.push_str(b);
    out
}

fn is_common_short(w: &str) -> bool {
    matches!(
        w,
        "the" | "and" | "for" | "you" | "not" | "new" | "all" | "one"
            | "two" | "out" | "now" | "but" | "are" | "how" | "who" | "why"
            | "can" | "has" | "was" | "his" | "her" | "its" | "our" | "any"
            | "get" | "use" | "see" | "day" | "way" | "may" | "say"
    )
}

fn is_allcaps(s: &str) -> bool {
    s.len() >= 2 && s.bytes().all(|b| b.is_ascii_uppercase())
}

fn collect_needed(
    user: &UserDict,
    tok: &str,
    allow_camel: bool,
    needed: &mut HashSet<String>,
) {
    if tok.is_empty() {
        return;
    }
    if tok.len() == 1 {
        if tok == "a" {
            needed.insert("a".to_string());
        }
        return;
    }
    if find_user(user, tok).is_some() {
        return;
    }

    let w = lower_ascii(tok);
    let n = strip_apostrophe(&w);

    if tok.len() <= 3 && is_allcaps(tok) && !is_common_short(&w) {
        return;
    }

    needed.insert(w.clone());
    needed.insert(n.clone());

    if w.len() > 3 && w.ends_with("'s") {
        let base_orig = &tok[..tok.len() - 2];
        if find_user(user, base_orig).is_none() {
            needed.insert(w[..w.len() - 2].to_string());
        }
    }

    for m in morph_bases(&n) {
        needed.insert(m.base);
    }

    if allow_camel {
        for p in split_camel(tok) {
            collect_needed(user, p, false, needed);
        }
    }

    if n.len() >= 6 && n.len() <= 24 {
        for k in 2..=n.len() - 2 {
            let left = &n[..k];
            let right = &n[k..];
            if left.len() >= 3 && has_en_vowel(left) && !user.lower_map.contains_key(left) {
                needed.insert(left.to_string());
            }
            if right.len() >= 3 && has_en_vowel(right) && !user.lower_map.contains_key(right) {
                needed.insert(right.to_string());
            }
        }
    }
}

fn lookup_part(
    user: &UserDict,
    cmu: &HashMap<String, Vec<String>>,
    part: &str,
) -> (String, bool) {
    if let Some(u) = user.lower_map.get(part) {
        return (u.clone(), true);
    }
    if part.len() < 3 || !has_en_vowel(part) {
        return (String::new(), false);
    }
    if let Some(ci) = cmu.get(part) {
        return (arpa_to_persian(ci), false);
    }
    (String::new(), false)
}

fn render_token(
    user: &UserDict,
    cmu: &HashMap<String, Vec<String>>,
    token: &str,
    allow_camel: bool,
) -> String {
    // 1. User dictionary match (exact case first, then case-insensitive)
    if let Some(u) = find_user(user, token) {
        return u.to_string();
    }

    // 2. Single letter (lowercase 'a' goes to CMUdict; 'A', 'B', 'I', ... are spelled)
    if token.len() == 1 && token != "a" {
        return spell_letters(token);
    }

    let w = lower_ascii(token);
    let n = strip_apostrophe(&w);
    let allcaps = is_allcaps(token);

    // 3. Short all-caps acronyms (CPU, API, GPU, OS, AI)
    if allcaps && token.len() <= 3 && !is_common_short(&w) {
        return spell_letters(token);
    }

    // 4. CMUdict exact match (with apostrophe intact)
    if let Some(ph) = cmu.get(&w) {
        let res = arpa_to_persian(ph);
        if !res.is_empty() {
            return res;
        }
    }

    // 5. Possessive 's — checked before stripping apostrophe
    if w.len() > 3 && w.ends_with("'s") {
        let base_orig = &token[..token.len() - 2];
        if let Some(ub) = find_user(user, base_orig) {
            return format!("{}ز", ub);
        }
        let base = &w[..w.len() - 2];
        if let Some(cb) = cmu.get(base) {
            if !cb.is_empty() {
                let bfa = arpa_to_persian(cb);
                if !bfa.is_empty() {
                    return format!("{}{}", bfa, morph_suffix(MorphKind::S, cb.last().unwrap()));
                }
            }
        }
    }

    // 6. CMUdict match without apostrophe
    if n != w {
        if let Some(ph) = cmu.get(&n) {
            let res = arpa_to_persian(ph);
            if !res.is_empty() {
                return res;
            }
        }
    }

    // 7. 4-5 letter all-caps acronyms not in CMUdict
    if allcaps && token.len() <= 5 {
        return spell_letters(token);
    }

    // 8. CamelCase splitting
    if allow_camel {
        let parts = split_camel(token);
        if parts.len() >= 2 {
            let mut combined = String::new();
            for p in parts {
                combined = join_parts(&combined, &render_token(user, cmu, p, false));
            }
            if !combined.is_empty() {
                return combined;
            }
        }
    }

    // 9. Inflected / derived forms via CMUdict base
    for m in morph_bases(&n) {
        if let Some(bi) = cmu.get(&m.base) {
            if bi.is_empty() {
                continue;
            }
            let base_fa = arpa_to_persian(bi);
            if base_fa.is_empty() {
                continue;
            }
            return format!("{}{}", base_fa, morph_suffix(m.kind, bi.last().unwrap()));
        }
    }

    // 10. 2-part compound words
    if n.len() >= 6 && n.len() <= 24 {
        let mut best_score: i32 = -1;
        let mut best_out = String::new();

        for k in 2..=n.len() - 2 {
            let left = &n[..k];
            let right = &n[k..];

            let (lfa, lu) = lookup_part(user, cmu, left);
            if lfa.is_empty() {
                continue;
            }
            let (rfa, ru) = lookup_part(user, cmu, right);
            if rfa.is_empty() {
                continue;
            }

            let score = (left.len().min(right.len()) as i32) * 10
                + if lu { 3 } else { 0 }
                + if ru { 3 } else { 0 };
            if score > best_score {
                best_score = score;
                best_out = join_parts(&lfa, &rfa);
            }
        }

        if !best_out.is_empty() {
            return best_out;
        }
    }

    // 11. Rule-based fallback
    let res = rule_based(&n);
    if res.is_empty() {
        spell_letters(&n)
    } else {
        res
    }
}

fn latin_to_persian(input: &str, cmu_path: &str, user_path: &str) -> String {
    let text = fold_latin(input).replace('’', "'");
    let b = text.as_bytes();

    struct Token {
        word: bool,
        value: String,
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut current = Vec::new();
    let mut in_word = false;

    let flush = |tokens: &mut Vec<Token>, current: &mut Vec<u8>, in_word: bool| {
        if !current.is_empty() {
            let s = String::from_utf8(std::mem::take(current)).unwrap_or_default();
            tokens.push(Token { word: in_word, value: s });
        }
    };

    for i in 0..b.len() {
        let c = b[i];
        if is_ascii_alpha(c) {
            if !in_word {
                flush(&mut tokens, &mut current, in_word);
                in_word = true;
            }
            current.push(c);
        } else if c == b'\'' && in_word && i + 1 < b.len() && is_ascii_alpha(b[i + 1]) {
            current.push(b'\'');
        } else {
            if in_word {
                flush(&mut tokens, &mut current, in_word);
                in_word = false;
            }
            current.push(c);
        }
    }
    flush(&mut tokens, &mut current, in_word);

    if !tokens.iter().any(|t| t.word) {
        return text;
    }

    let user = load_user_dict(user_path);
    let mut needed = HashSet::new();
    for t in &tokens {
        if t.word {
            collect_needed(&user, &t.value, true, &mut needed);
        }
    }

    let cmu = load_cmu(cmu_path, &needed);
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut out = String::with_capacity(text.len() * 2);

    for t in tokens {
        if !t.word {
            out.push_str(&t.value);
            continue;
        }
        let rendered = cache
            .entry(t.value.clone())
            .or_insert_with(|| render_token(&user, &cmu, &t.value, true));
        out.push_str(rendered);
    }

    out
}

// ------------------------------------------------------------
// Symbol normalization
// ------------------------------------------------------------

fn normalize_symbols(input: &str) -> String {
    let mut text = input.to_string();
    for &(a, b) in &[
        ("&", " و "),
        ("+", " په‌لاس "),
        ("=", " مساوی "),
        ("$", " دلار "),
        ("<", " کوچکتر از "),
        (">", " بزرگتر از "),
        ("@", " ات‌ساین "),
    ] {
        if text.contains(a) {
            text = text.replace(a, b);
        }
    }

    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.'
            && i > 0
            && i + 1 < b.len()
            && is_ascii_alpha(b[i - 1])
            && is_ascii_alpha(b[i + 1])
        {
            out.push_str(" دات ");
            i += 1;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    text = out;

    for s in ["*", "_", "~", "^", "|", "\\", "{", "}", "`"] {
        if text.contains(s) {
            text = text.replace(s, " ");
        }
    }

    let b = text.as_bytes();
    let mut out2 = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' {
            let glued = i + 1 < b.len()
                && b[i + 1] != b' '
                && b[i + 1] != b'#'
                && b[i + 1] != b'\n';
            out2.push_str(if glued { " هشتگ " } else { " " });
            i += 1;
        } else {
            let ch = text[i..].chars().next().unwrap();
            out2.push(ch);
            i += ch.len_utf8();
        }
    }

    out2
}

// ------------------------------------------------------------
// Final whitespace normalization
// ------------------------------------------------------------

fn finish(text: &str) -> String {
    const ZW: char = '\u{200C}';

    let is_letter = |c: char| -> bool {
        "آابپتثجچحخدذرزژسشصضطظعغفقکگلمنوهیئ".contains(c)
    };

    let is_punct = |c: char| -> bool {
        matches!(c, '.' | '،' | ':' | '؛' | '!' | '؟')
    };

    let mut out: Vec<char> = Vec::with_capacity(text.len());
    for c in text.chars() {
        if c == ZW
            || is_letter(c)
            || is_punct(c)
            || c == ' '
            || c.is_ascii_alphanumeric()
        {
            out.push(c);
        } else if c == '…' {
            out.push('.');
        } else {
            out.push(' ');
        }
    }

    let mut compact: Vec<char> = Vec::with_capacity(out.len());
    let mut i = 0;
    while i < out.len() {
        if out[i] == ' ' || out[i] == ZW {
            let mut j = i;
            let mut has_space = false;
            while j < out.len() && (out[j] == ' ' || out[j] == ZW) {
                if out[j] == ' ' {
                    has_space = true;
                }
                j += 1;
            }
            compact.push(if has_space { ' ' } else { ZW });
            i = j;
        } else {
            compact.push(out[i]);
            i += 1;
        }
    }

    out.clear();
    for i in 0..compact.len() {
        if compact[i] == ZW {
            if i > 0
                && i + 1 < compact.len()
                && is_letter(compact[i - 1])
                && is_letter(compact[i + 1])
            {
                out.push(ZW);
            }
        } else {
            out.push(compact[i]);
        }
    }

    compact.clear();
    for c in out {
        if is_punct(c) {
            while compact.last() == Some(&' ') {
                compact.pop();
            }
            if compact.last() == Some(&c) {
                continue;
            }
        } else if c == ' ' && (compact.is_empty() || compact.last() == Some(&' ')) {
            continue;
        }
        compact.push(c);
    }

    while compact.last() == Some(&' ') {
        compact.pop();
    }

    let mut start = 0;
    while start < compact.len() && (compact[start] == ' ' || is_punct(compact[start])) {
        start += 1;
    }

    compact[start..].iter().collect()
}

// ------------------------------------------------------------
// Data path
// ------------------------------------------------------------

fn data_path(env_key: &str, name: &str) -> String {
    if let Ok(v) = env::var(env_key) {
        if !v.is_empty() {
            return v;
        }
    }
    if Path::new(name).is_file() {
        return name.to_string();
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(name);
            if cand.is_file() {
                if let Some(s) = cand.to_str() {
                    return s.to_string();
                }
            }
        }
    }
    name.to_string()
}

// ------------------------------------------------------------
// Main normalization pipeline
// ------------------------------------------------------------

pub fn normalize_with_dicts(
    input: &str,
    cmu_path: Option<&str>,
    user_path: Option<&str>,
) -> String {
    let text = clean_persian(input);
    let text = normalize_symbols(&text);
    let cmu = cmu_path
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| data_path("G2P_CMUDICT", "cmudict.txt"));
    let user = user_path
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| data_path("G2P_LATIN_DICT", "latin_dict.txt"));
    let text = latin_to_persian(&text, &cmu, &user);
    finish(&text)
}

pub fn normalize(input: &str) -> String {
    normalize_with_dicts(input, None, None)
}

// ------------------------------------------------------------
// C / C++ / FFI API
// ------------------------------------------------------------

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

unsafe fn cstr_opt<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

#[no_mangle]
pub unsafe extern "C" fn persian_normalize(
    text: *const c_char,
    cmudict_path: *const c_char,
    latin_dict_path: *const c_char,
) -> *mut c_char {
    let Some(input) = cstr_opt(text) else {
        return ptr::null_mut();
    };
    let out = normalize_with_dicts(input, cstr_opt(cmudict_path), cstr_opt(latin_dict_path));
    match CString::new(out.replace('\0', "")) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn persian_normalize_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
