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
}
