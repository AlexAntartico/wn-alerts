pub fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
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
}
