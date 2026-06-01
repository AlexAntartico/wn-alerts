pub fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Strip all HTML tags from `s`, converting block-level tags to newlines.
/// Remaining HTML entities are decoded so the result is plain text.
pub fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();

    for ch in s.chars() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if ch == '>' && in_tag {
            in_tag = false;
            let lower = tag_buf.trim().to_ascii_lowercase();
            // Strip leading '/' so closing tags match the same list
            let name = lower
                .trim_start_matches('/')
                .split(|c: char| !c.is_ascii_alphabetic())
                .next()
                .unwrap_or("");
            if matches!(name, "p" | "br" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                out.push('\n');
            }
        } else if in_tag {
            tag_buf.push(ch);
        } else {
            out.push(ch);
        }
    }

    // Decode entities in the tag-stripped text
    let decoded = decode_html_entities(out.trim());

    // Collapse runs of 3+ newlines down to 2
    let mut clean = String::with_capacity(decoded.len());
    let mut newline_run = 0usize;
    for ch in decoded.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                clean.push(ch);
            }
        } else {
            newline_run = 0;
            clean.push(ch);
        }
    }

    clean.trim().to_string()
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Truncate `s` to at most `max_chars` Unicode scalar values without splitting
/// an HTML entity (e.g. `&amp;`). If the cut point falls inside an entity — an
/// unmatched `&` with no closing `;` in the kept slice — we back up to just
/// before that `&` so the result stays valid for Telegram's HTML parse mode.
pub fn truncate_html_safe(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let head = &s[..end];
    if let Some(amp) = head.rfind('&') {
        if !head[amp..].contains(';') {
            end = amp;
        }
    }
    s[..end].to_string()
}

pub fn format_pub_date(rfc2822: &str) -> String {
    chrono::DateTime::parse_from_rfc2822(rfc2822)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| rfc2822.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_all_standard_entities() {
        let input = "&amp; &lt; &gt; &quot; &#39; &nbsp;";
        let expected = "& < > \" '  ";
        assert_eq!(decode_html_entities(input), expected);
    }

    #[test]
    fn decode_plain_text_passes_through() {
        assert_eq!(decode_html_entities("No entities"), "No entities");
    }

    #[test]
    fn escape_special_chars() {
        let input = "<b>hello & world</b>";
        let expected = "&lt;b&gt;hello &amp; world&lt;/b&gt;";
        assert_eq!(escape_html(input), expected);
    }

    #[test]
    fn escape_plain_text_passes_through() {
        assert_eq!(escape_html("No special characters"), "No special characters");
    }

    #[test]
    fn format_valid_rfc2822() {
        let result = format_pub_date("Thu, 21 May 2026 18:00:00 GMT");
        assert_eq!(result, "2026-05-21 18:00 UTC");
    }

    #[test]
    fn format_invalid_date_returns_original() {
        assert_eq!(format_pub_date("not a date"), "not a date");
    }

    #[test]
    fn format_empty_string() {
        assert_eq!(format_pub_date(""), "");
    }

    #[test]
    fn strip_inline_tags_preserves_text() {
        assert_eq!(strip_html_tags("<b>bold</b> and <i>italic</i>"), "bold and italic");
    }

    #[test]
    fn strip_block_tags_adds_newlines() {
        assert_eq!(strip_html_tags("<p>First</p><p>Second</p>"), "First\n\nSecond");
    }

    #[test]
    fn strip_br_adds_newline() {
        assert_eq!(strip_html_tags("line one<br>line two"), "line one\nline two");
        assert_eq!(strip_html_tags("line one<br />line two"), "line one\nline two");
    }

    #[test]
    fn strip_decodes_entities_in_text() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("<p>a &amp; b</p>"), "a & b");
    }

    #[test]
    fn strip_collapses_excessive_newlines() {
        let input = "<p>A</p><p></p><p></p><p>B</p>";
        let result = strip_html_tags(input);
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn strip_var_tags_keeps_content() {
        let input = "Jun <var data-var='date'>29</var>, <var data-var='time'>06:00</var>";
        assert_eq!(strip_html_tags(input), "Jun 29, 06:00");
    }

    #[test]
    fn strip_twilio_style_description() {
        let input = "<p><strong>SCHEDULED EVENT Jun <var data-var='date'>29</var></strong></p> \
                     <p><small>May 28</small><br>Maintenance window.</p>";
        let result = strip_html_tags(input);
        assert!(result.contains("SCHEDULED EVENT Jun 29"));
        assert!(result.contains("May 28"));
        assert!(result.contains("Maintenance window."));
        assert!(!result.contains('<'));
        assert!(!result.contains('>'));
    }

    #[test]
    fn strip_empty_string() {
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn strip_plain_text_passes_through() {
        assert_eq!(strip_html_tags("No tags here"), "No tags here");
    }

    #[test]
    fn truncate_under_limit_passes_through() {
        assert_eq!(truncate_html_safe("hello", 10), "hello");
        assert_eq!(truncate_html_safe("hello", 5), "hello");
    }

    #[test]
    fn truncate_cuts_to_max_chars() {
        assert_eq!(truncate_html_safe("abcdef", 3), "abc");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // Each 'é' is 2 bytes but 1 char; cutting at 2 chars must not split it.
        assert_eq!(truncate_html_safe("éééé", 2), "éé");
    }

    #[test]
    fn truncate_does_not_split_entity() {
        // "ab&amp;cd" -> cutting mid-entity drops the whole "&amp;".
        assert_eq!(truncate_html_safe("ab&amp;cd", 4), "ab");
        assert_eq!(truncate_html_safe("ab&amp;cd", 5), "ab");
        assert_eq!(truncate_html_safe("ab&amp;cd", 6), "ab");
    }

    #[test]
    fn truncate_keeps_complete_entity() {
        // Cutting exactly at the ';' keeps the full entity.
        assert_eq!(truncate_html_safe("ab&amp;cd", 7), "ab&amp;");
    }
}
