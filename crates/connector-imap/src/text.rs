//! Turning a mail body into the text a person and a model should read.
//!
//! Design: `docs/design/01-data-model.md`, "text 是归一纯文本，不是原文".
//!
//! Two jobs, both lossy on purpose, and both with the original kept in the
//! raw record so nothing is actually lost. Strip the markup from an HTML
//! mail, and take off the quoted copy of the message being replied to.
//!
//! The rule the design sets for quoting is to be conservative: keeping too
//! much only makes a summary slightly worse, while cutting too much silently
//! deletes what somebody said. So a quote is removed when its marker is
//! unmistakable, and left alone otherwise.

/// Convert an HTML body to plain text.
///
/// Not a renderer. It drops what carries no meaning for a reader, keeps the
/// text and the link targets, and turns block structure into line breaks.
/// Tracking pixels and the rest of the invisible payload go with the markup.
#[must_use]
pub fn from_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'<' {
                i += 1;
            }
            push_text(&mut out, &html[start..i]);
            continue;
        }

        let Some(end) = html[i..].find('>').map(|e| i + e) else {
            break;
        };
        let tag = &html[i + 1..end];
        let name = tag_name(tag);

        match name.as_str() {
            // Content nobody is meant to read.
            "script" | "style" | "head" | "title" => {
                i = skip_element(html, end + 1, &name);
                continue;
            }
            // A link's target is part of what it says, so keep it when it is
            // not simply the text again.
            "a" => {
                if let Some(href) = attribute(tag, "href")
                    && !href.starts_with("mailto:")
                    && !href.starts_with('#')
                {
                    let (text, after) = element_text(html, end + 1, "a");
                    let text = text.trim();
                    if text.is_empty() {
                        push_text(&mut out, &href);
                    } else if text == href {
                        push_text(&mut out, text);
                    } else {
                        push_text(&mut out, &format!("{text} <{href}>"));
                    }
                    i = after;
                    continue;
                }
            }
            "br" => out.push('\n'),
            "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
            | "table" | "ul" | "ol" | "hr" => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            "td" | "th" if !out.is_empty() && !out.ends_with([' ', '\n']) => out.push(' '),
            _ => {}
        }
        i = end + 1;
    }

    tidy(&out)
}

/// Remove the quoted copy of an earlier message.
///
/// Conservative by design: it cuts at an unmistakable marker and nowhere
/// else, and it refuses to cut when doing so would leave nothing, because a
/// mail that is entirely a quote is one where the quote is the content.
#[must_use]
pub fn strip_quote(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    let mut cut = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_quote_marker(trimmed) || starts_attribution(trimmed, lines.get(index + 1).copied()) {
            cut = Some(index);
            break;
        }
    }

    let kept = match cut {
        Some(index) => lines[..index].join("\n"),
        None => text.to_owned(),
    };

    // Everything was quoted. Then the quote is the message.
    if kept.trim().is_empty() {
        return tidy(text);
    }
    tidy(&strip_signature(&kept))
}

/// Whether a line is one of the separators mail clients put above a quote.
fn is_quote_marker(line: &str) -> bool {
    const MARKERS: [&str; 8] = [
        "-----original message-----",
        "-----原始邮件-----",
        "________________________________",
        "-------- forwarded message --------",
        "---------- forwarded message ----------",
        "-------- 转发的邮件 --------",
        "begin forwarded message:",
        "发件人:",
    ];
    let lower = line.to_lowercase();
    MARKERS.iter().any(|m| lower.starts_with(m))
}

/// Whether a line is the "X wrote:" sentence a client puts above a quote.
///
/// Requiring the quoted block to follow is what keeps this from eating a
/// sentence that merely ends in a colon.
fn starts_attribution(line: &str, next: Option<&str>) -> bool {
    let lower = line.to_lowercase();
    let looks_like_attribution = (lower.starts_with("on ") && lower.ends_with("wrote:"))
        || (lower.starts_with("在") && lower.ends_with("写道："))
        || (lower.starts_with("于") && lower.ends_with("写道："));
    if !looks_like_attribution {
        return false;
    }
    // A quote follows, or the attribution is the last thing in the mail.
    next.is_none_or(|n| n.trim().is_empty() || n.trim_start().starts_with('>'))
}

/// Take off a signature block, which by convention starts with `-- `.
fn strip_signature(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    if let Some(index) = lines.iter().position(|l| l.trim_end() == "--") {
        lines.truncate(index);
    }
    lines.join("\n")
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .split([' ', '\t', '\n', '/', '>'])
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)? + 1;
        Some(rest[1..end].to_owned())
    } else {
        let end = rest.find([' ', '>']).unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    }
}

/// Skip past a whole element, used for content nobody reads.
fn skip_element(html: &str, from: usize, name: &str) -> usize {
    let closing = format!("</{name}");
    match html[from..].to_lowercase().find(&closing) {
        Some(at) => html[from + at..]
            .find('>')
            .map_or(html.len(), |e| from + at + e + 1),
        None => html.len(),
    }
}

/// The text inside an element, and where it ends.
fn element_text(html: &str, from: usize, name: &str) -> (String, usize) {
    let closing = format!("</{name}");
    let Some(at) = html[from..].to_lowercase().find(&closing) else {
        return (String::new(), html.len());
    };
    let inner = &html[from..from + at];
    let after = html[from + at..]
        .find('>')
        .map_or(html.len(), |e| from + at + e + 1);
    (from_html(inner), after)
}

/// Append text, decoding entities and collapsing runs of whitespace.
fn push_text(out: &mut String, raw: &str) {
    let decoded = decode_entities(raw);
    for ch in decoded.chars() {
        if is_invisible(ch) {
            continue;
        }
        if ch.is_whitespace() && ch != '\n' {
            if !out.ends_with([' ', '\n']) {
                out.push(' ');
            }
        } else if ch == '\n' {
            if !out.ends_with('\n') {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
    }
}

/// Zero-width and joining characters: present in the bytes, absent on the
/// screen, so absent from the text too.
fn is_invisible(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}' | '\u{034F}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}'
    )
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        // Byte positions, because a fixed byte window can end inside a
        // multi-byte character; a `;` is ASCII, so its position is always a
        // boundary.
        let Some(end) = tail.bytes().take(12).position(|b| b == b';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            // Invisible joiners, which marketing mail pads its preview text
            // with. They render as nothing, so they become nothing.
            "zwnj" | "zwj" | "shy" => Some('\u{200B}'),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            other => other
                .strip_prefix('#')
                .and_then(|n| {
                    n.strip_prefix('x')
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .or_else(|| n.parse().ok())
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => out.push(ch),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Trim each line, drop runs of blank lines, trim the whole.
fn tidy(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() && lines.last().is_some_and(|l: &String| l.is_empty()) {
            continue;
        }
        lines.push(trimmed.to_owned());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ampersand_followed_by_multibyte_text_does_not_panic() {
        // The twelve-byte window after `&` used to be sliced by byte, which
        // panicked when it landed inside a character.
        assert_eq!(
            from_html("a &b\u{200c}\u{200c}\u{200c}\u{200c} c"),
            "a &b c"
        );
        assert_eq!(from_html("&中文中文中文;"), "&中文中文中文;");
        assert_eq!(from_html("&zwnj;&#847;x"), "x");
    }

    #[test]
    fn markup_becomes_lines_and_text() {
        let html = "<html><body><p>Hi Neo,</p><p>Two things:</p>\
                    <ul><li>the timeline</li><li>the support terms</li></ul>\
                    <p>Thanks,<br>Maria</p></body></html>";
        assert_eq!(
            from_html(html),
            "Hi Neo,\nTwo things:\nthe timeline\nthe support terms\nThanks,\nMaria"
        );
    }

    #[test]
    fn what_nobody_reads_is_dropped() {
        let html = "<head><title>ignored</title></head><body>\
                    <style>p{color:red}</style>visible\
                    <script>track('open')</script>\
                    <img src=\"https://tracker.example/pixel.gif?id=42\" width=1 height=1>\
                    </body>";
        let text = from_html(html);
        assert_eq!(text, "visible");
        assert!(!text.contains("tracker.example"), "{text}");
        assert!(!text.contains("color"), "{text}");
    }

    #[test]
    fn a_link_keeps_its_target_when_that_says_something_extra() {
        assert_eq!(
            from_html(r#"see <a href="https://example.com/doc">the document</a> please"#),
            "see the document <https://example.com/doc> please"
        );
        assert_eq!(
            from_html(r#"<a href="https://example.com">https://example.com</a>"#),
            "https://example.com",
            "no point saying it twice"
        );
        assert_eq!(
            from_html(r#"write to <a href="mailto:me@example.com">me</a>"#),
            "write to me"
        );
    }

    #[test]
    fn entities_come_back_as_characters() {
        assert_eq!(
            from_html("Tom &amp; Jerry &mdash; &quot;quoted&quot; &#8212; &#x4e2d;&#x6587;"),
            "Tom & Jerry — \"quoted\" — 中文"
        );
    }

    #[test]
    fn a_quoted_reply_is_removed_but_the_answer_is_kept() {
        let body = "Friday works for me.\n\n\
                    On Mon, 1 Sep 2026 at 10:00, Maria <maria@example.com> wrote:\n\
                    > Can we move milestone two?\n> Thanks";
        assert_eq!(strip_quote(body), "Friday works for me.");
    }

    #[test]
    fn a_separator_line_stands_on_its_own() {
        for marker in [
            "-----Original Message-----",
            "-----原始邮件-----",
            "________________________________",
            "-------- Forwarded message --------",
            "发件人: Maria <maria@example.com>",
        ] {
            let body = format!("my reply\n\n{marker}\nolder text here");
            assert_eq!(strip_quote(&body), "my reply", "marker: {marker}");
        }
    }

    #[test]
    fn an_attribution_needs_the_quote_it_introduces() {
        // Separators are unambiguous on their own. An attribution is a
        // sentence, and sentences can end in a colon for other reasons, so
        // it only counts when a quote follows.
        for attribution in [
            "On Mon, 1 Sep 2026 at 09:00, Maria wrote:",
            "在 2026年9月1日，Maria 写道：",
            "于 2026年9月1日，Maria 写道：",
        ] {
            let quoted = format!("my reply\n\n{attribution}\n> the older text");
            assert_eq!(
                strip_quote(&quoted),
                "my reply",
                "with a quote: {attribution}"
            );

            let bare = format!("my reply\n\n{attribution}\nsomething that is not a quote");
            assert_eq!(
                strip_quote(&bare),
                bare.trim(),
                "without a quote it is just a sentence: {attribution}"
            );
        }
    }

    #[test]
    fn a_sentence_ending_in_a_colon_is_not_a_quote_marker() {
        let body = "On the question of scope, here is what I wrote:\n\
                    we should keep it narrow and ship.";
        assert_eq!(
            strip_quote(body),
            body,
            "an attribution needs a quote after it"
        );
    }

    #[test]
    fn a_mail_that_is_entirely_a_quote_keeps_its_quote() {
        let body = "On Mon, 1 Sep 2026, Maria wrote:\n> the whole message";
        assert_eq!(
            strip_quote(body),
            body,
            "cutting everything would delete the content"
        );
    }

    #[test]
    fn a_signature_is_taken_off() {
        let body = "See you Thursday.\n\n-- \nMaria Alvarez\nContoso";
        assert_eq!(strip_quote(body), "See you Thursday.");
    }

    #[test]
    fn a_dashed_line_that_is_not_a_signature_stays() {
        let body = "Options:\n--- one ---\n--- two ---";
        assert_eq!(strip_quote(body), body);
    }

    #[test]
    fn whitespace_is_tidied_without_losing_paragraphs() {
        assert_eq!(
            from_html("<p>first</p>\n\n\n<p>  second  </p>"),
            "first\nsecond"
        );
        assert_eq!(strip_quote("  padded  \n\n\n\nnext  "), "padded\n\nnext");
    }

    #[test]
    fn unterminated_markup_does_not_run_away() {
        assert_eq!(from_html("<p>text<"), "text");
        assert_eq!(from_html("<script>never closed"), "");
        assert_eq!(from_html(""), "");
    }
}
