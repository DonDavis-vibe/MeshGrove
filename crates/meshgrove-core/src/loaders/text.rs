//! Readmes, licences and notes: decoding as editors guess it, and a thumbnail that looks like
//! the first page (Markdown headings larger, links and badges reduced to their text).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{Rgba, RgbaImage};

/// Only the start of a file shows on a thumbnail.
const HEAD: usize = 16 * 1024;
const PAPER: Rgba<u8> = Rgba([247, 245, 240, 255]);
const EDGE: Rgba<u8> = Rgba([214, 210, 202, 255]);
const INK: [u8; 3] = [70, 70, 72];
const HEADING_INK: [u8; 3] = [20, 20, 24];

/// Markdown by its extension (`.md`, `.markdown`).
pub fn is_markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// UTF-8 (with or without BOM), UTF-16 with BOM, anything else as Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&c| from(c)).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            // Cut off in the middle of a character (the head of a file): still UTF-8.
            Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
            Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
        },
    }
}

/// One line on the thumbnail page; `level` 1–6 for Markdown headings, 0 for body text.
#[derive(Debug, Clone, PartialEq)]
struct PageLine {
    level: u8,
    text: String,
}

/// The lines a thumbnail shows: tabs expanded, control characters dropped, and for Markdown the
/// markup removed. Lines that held only markup (badges, HTML) disappear; blank lines stay as
/// paragraph gaps, one at a time.
fn page_lines(text: &str, markdown: bool) -> Vec<PageLine> {
    let mut out: Vec<PageLine> = Vec::new();
    let blank = PageLine { level: 0, text: String::new() };
    for raw in text.lines() {
        let line: String =
            raw.replace('\t', "    ").chars().filter(|c| !c.is_control()).collect::<String>().trim_end().to_owned();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if out.last().is_some_and(|l| !l.text.is_empty()) {
                out.push(blank.clone());
            }
            continue;
        }
        if !markdown {
            out.push(PageLine { level: 0, text: line });
            continue;
        }
        // Code fences and underlines of headings carry no words.
        if trimmed.starts_with("```") || trimmed.chars().all(|c| matches!(c, '=' | '-' | '*' | '_' | ' ')) {
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        let (level, body) = if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            (hashes as u8, trimmed[hashes..].trim())
        } else {
            (0, line.as_str())
        };
        let body = body.trim_start_matches("> ");
        let indent = body.len() - body.trim_start().len();
        let rest = body.trim_start();
        let body = match rest.get(..2) {
            Some("- " | "* " | "+ ") => format!("{}• {}", " ".repeat(indent), &rest[2..]),
            _ => body.to_owned(),
        };
        let text = strip_inline(&body);
        if !text.trim().is_empty() {
            out.push(PageLine { level, text: text.trim_end().to_owned() });
        }
    }
    out
}

/// Inline Markdown and HTML reduced to the words: `[text](url)` → text, images and tags
/// removed, emphasis and code marks dropped, the common entities decoded.
fn strip_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '!'
            && rest[1..].starts_with('[')
            && let Some((_, len)) = link(&rest[1..])
        {
            rest = &rest[1 + len..];
            continue;
        }
        if c == '['
            && let Some((text, len)) = link(rest)
        {
            out.push_str(&strip_inline(text));
            rest = &rest[len..];
            continue;
        }
        if c == '<'
            && rest[1..].starts_with(|n: char| n.is_ascii_alphabetic() || n == '/' || n == '!')
            && let Some(end) = rest.find('>')
        {
            rest = &rest[end + 1..];
            continue;
        }
        if let Some(mark) = ["**", "__", "~~"].into_iter().find(|m| rest.starts_with(m)) {
            rest = &rest[mark.len()..];
            continue;
        }
        if c == '`' {
            rest = &rest[1..];
            continue;
        }
        if c == '&'
            && let Some((entity, ch)) = [("&nbsp;", ' '), ("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>'), ("&quot;", '"')]
                .into_iter()
                .find(|(e, _)| rest.starts_with(e))
        {
            out.push(ch);
            rest = &rest[entity.len()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Markdown for a viewer that neither renders HTML nor loads pictures: images become their alt
/// text (a badge link keeps its link), HTML tags are dropped, autolinks stay. Code blocks and
/// code spans are left as they are.
pub fn display_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut fenced = false;
    for line in md.split_inclusive('\n') {
        let start = line.trim_start();
        if start.starts_with("```") || start.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
        } else if fenced || line.starts_with("    ") || line.starts_with('\t') {
            out.push_str(line);
        } else {
            out.push_str(&without_html(line));
        }
    }
    out
}

fn without_html(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            let ticks = rest.chars().take_while(|&t| t == '`').count();
            let len = rest[ticks..].find(&rest[..ticks]).map_or(ticks, |end| 2 * ticks + end);
            out.push_str(&rest[..len]);
            rest = &rest[len..];
            continue;
        }
        if c == '!'
            && rest[1..].starts_with('[')
            && let Some((alt, len)) = link(&rest[1..])
        {
            out.push_str(alt);
            rest = &rest[1 + len..];
            continue;
        }
        if c == '<'
            && rest[1..].starts_with(|n: char| n.is_ascii_alphabetic() || n == '/' || n == '!')
            && let Some(end) = rest.find('>')
        {
            let tag = &rest[1..end];
            if tag.contains("://") || (tag.contains('@') && !tag.contains(' ')) {
                out.push_str(&rest[..=end]); // <https://…> and <mail@…> are links
            }
            rest = &rest[end + 1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `[text](target)` at the start of `s`: the text and the length of the whole link.
fn link(s: &str) -> Option<(&str, usize)> {
    let close = matching(s, '[', ']')?;
    let after = &s[close + 1..];
    if !after.starts_with('(') {
        return None;
    }
    let end = matching(after, '(', ')')?;
    Some((&s[1..close], close + 1 + end + 1))
}

/// Byte index of the bracket closing the one `s` starts with.
fn matching(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// The first page of the text as a picture `size` pixels high, on paper (a blank page for an
/// empty file). `None` only if the bundled font cannot be read.
pub fn thumbnail(bytes: &[u8], markdown: bool, size: u32) -> Option<RgbaImage> {
    let text = decode(&bytes[..bytes.len().min(HEAD)]);
    let lines = page_lines(&text, markdown);
    let font = FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR).ok()?;
    let (w, h) = (((size as f32) * 0.77).round() as u32, size);
    let mut image = RgbaImage::from_pixel(w, h, PAPER);
    for x in 0..w {
        image.put_pixel(x, 0, EDGE);
        image.put_pixel(x, h - 1, EDGE);
    }
    for y in 0..h {
        image.put_pixel(0, y, EDGE);
        image.put_pixel(w - 1, y, EDGE);
    }
    let margin = w as f32 * 0.08;
    let body = h as f32 / 24.0;
    let width = w as f32 - 2.0 * margin;
    let bottom = h as f32 - margin * 0.6;
    let mut y = margin;
    for line in lines.iter().skip_while(|l| l.text.trim().is_empty()) {
        let px = match line.level {
            0 => body,
            1 => body * 1.6,
            2 => body * 1.3,
            _ => body * 1.12,
        };
        if line.text.trim().is_empty() {
            y += px * 0.6;
            continue;
        }
        let scaled = font.as_scaled(PxScale::from(px));
        let advance = scaled.h_advance(font.glyph_id('M')).max(1.0);
        let per_row = ((width / advance).floor() as usize).max(1);
        let color = if line.level > 0 { HEADING_INK } else { INK };
        for row in wrap(&line.text, per_row) {
            if y + px * 1.3 > bottom {
                return Some(image);
            }
            draw_row(&mut image, &font, px, margin, y + scaled.ascent(), &row, color);
            y += px * 1.3;
        }
        if line.level > 0 {
            y += px * 0.3;
        }
    }
    Some(image)
}

/// Rows of at most `per_row` characters (a monospaced font), broken at spaces where possible.
fn wrap(text: &str, per_row: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut rows = Vec::new();
    let mut start = 0;
    while chars.len() - start > per_row {
        let window = &chars[start..start + per_row + 1];
        let cut = window.iter().rposition(|&c| c == ' ').filter(|&i| i > 0).unwrap_or(per_row);
        rows.push(chars[start..start + cut].iter().collect());
        start += cut;
        while chars.get(start) == Some(&' ') {
            start += 1;
        }
    }
    rows.push(chars[start..].iter().collect());
    rows
}

fn draw_row(image: &mut RgbaImage, font: &FontRef, px: f32, x: f32, baseline: f32, row: &str, color: [u8; 3]) {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut pen = x;
    for c in row.chars() {
        let id = font.glyph_id(c);
        let glyph = id.with_scale_and_position(px, point(pen, baseline));
        pen += scaled.h_advance(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let (px, py) = (bounds.min.x as i64 + gx as i64, bounds.min.y as i64 + gy as i64);
            if px < 0 || py < 0 || px >= image.width() as i64 || py >= image.height() as i64 {
                return;
            }
            let dst = image.get_pixel_mut(px as u32, py as u32);
            let a = coverage.clamp(0.0, 1.0);
            for i in 0..3 {
                dst[i] = (f32::from(dst[i]) * (1.0 - a) + f32::from(color[i]) * a).round() as u8;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_are_guessed() {
        assert_eq!(decode(b"\xEF\xBB\xBFGr\xC3\xBC\xC3\x9Fe"), "Grüße");
        assert_eq!(decode(b"\xFF\xFEH\x00i\x00"), "Hi");
        assert_eq!(decode(b"\xFE\xFF\x00H\x00i"), "Hi");
        assert_eq!(decode(b"Gr\xFC\xDFe"), "Grüße", "Latin-1");
        assert_eq!(decode(b"Gr\xC3"), "Gr", "UTF-8 cut in the middle of a character");
    }

    #[test]
    fn markdown_is_reduced_to_its_words() {
        let md = "# Hexa LED\n\n[![Build](https://x/badge.svg)](https://x)\n\n<p align=\"center\"><img src=\"a.png\"></p>\n\n\
                  Print **two** of [these](https://x) with `PLA` &amp; enjoy.\n- one\n  * two\n## Parts\n---\n```\ncode\n```";
        let lines = page_lines(md, true);
        let texts: Vec<(u8, &str)> = lines.iter().map(|l| (l.level, l.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                (1, "Hexa LED"),
                (0, ""),
                (0, "Print two of these with PLA & enjoy."),
                (0, "• one"),
                (0, "  • two"),
                (2, "Parts"),
                (0, "code"),
            ]
        );
        assert_eq!(page_lines("[x] done\n**", false)[0].text, "[x] done", "plain text stays as it is");
        assert_eq!(strip_inline("[x] done"), "[x] done", "no link without a target");
    }

    #[test]
    fn markdown_for_display_has_no_html_or_pictures() {
        let md = "<p align=\"center\"><img src=\"a.png\"></p>\n\
                  [![CI](https://x/b.svg)](https://x) see <https://x.org> or ![photo](p.jpg)\n\
                  Use `<filament>` here\n```\n<keep>\n```\n    <indented>\n";
        assert_eq!(
            display_markdown(md),
            "\n[CI](https://x) see <https://x.org> or photo\nUse `<filament>` here\n```\n<keep>\n```\n    <indented>\n"
        );
    }

    #[test]
    fn rows_break_at_spaces() {
        assert_eq!(wrap("print the box twice", 10), ["print the", "box twice"]);
        assert_eq!(wrap("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(wrap("", 5), [""]);
    }

    #[test]
    fn thumbnail_is_a_page_with_ink() {
        let page = thumbnail(b"# Title\n\nSome words on the first page.", true, 200).unwrap();
        assert_eq!(page.dimensions(), (154, 200));
        assert_eq!(*page.get_pixel(5, 199), EDGE);
        let inked = page.pixels().filter(|p| p[0] < 150).count();
        assert!(inked > 50, "text is drawn ({inked} dark pixels)");
        let blank = |page: RgbaImage| page.pixels().all(|p| *p == PAPER || *p == EDGE);
        assert!(blank(thumbnail(b"  \n\n", false, 200).unwrap()), "an empty file is a blank page");
        assert!(blank(thumbnail(b"[![b](x)](y)\n", true, 200).unwrap()), "only a badge");
    }
}
