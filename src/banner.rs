use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const SM_JSON: &str = include_str!("../assets/smblock.json");
const BIT_JSON: &str = include_str!("../assets/font5x7.json");

#[derive(Deserialize)]
struct SmFile {
    height: usize,
    hardblank: char,
    glyphs: HashMap<String, Vec<String>>,
}

struct SmFont {
    height: usize,
    hardblank: char,
    glyphs: HashMap<char, Vec<String>>,
}

#[derive(Deserialize)]
struct BitFile {
    glyphs: HashMap<String, Vec<String>>,
}

struct BitFont {
    glyphs: HashMap<char, Vec<String>>,
}

fn sm() -> &'static SmFont {
    static FONT: OnceLock<SmFont> = OnceLock::new();
    FONT.get_or_init(|| {
        let parsed: SmFile = serde_json::from_str(SM_JSON).expect("smblock.json");
        let mut glyphs = HashMap::new();
        for (key, mut rows) in parsed.glyphs {
            let Some(ch) = key.chars().next() else {
                continue;
            };
            // Keep a little air between square brackets and the letters they
            // wrap; the block font otherwise kerns them right into the glyph.
            // The padding is a non-breaking space so kerning leaves it alone;
            // it turns into a normal space once the line is composed.
            match ch {
                '[' | ']' => {
                    for row in &mut rows {
                        row.insert(0, '\u{a0}');
                        row.push('\u{a0}');
                    }
                }
                _ => {}
            }
            glyphs.insert(ch, rows);
        }
        SmFont {
            height: parsed.height,
            hardblank: parsed.hardblank,
            glyphs,
        }
    })
}

fn bit() -> &'static BitFont {
    static FONT: OnceLock<BitFont> = OnceLock::new();
    FONT.get_or_init(|| {
        let parsed: BitFile = serde_json::from_str(BIT_JSON).expect("font5x7.json");
        let mut glyphs = HashMap::new();
        for (key, rows) in parsed.glyphs {
            if let Some(ch) = key.chars().next() {
                glyphs.insert(ch, rows);
            }
        }
        BitFont { glyphs }
    })
}

fn row_chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// Compose a line with the smblock font, kerning glyphs like FIGlet.
fn compose_sm(text: &str) -> Vec<String> {
    let font = sm();
    let mut buf: Vec<Vec<char>> = vec![Vec::new(); font.height];
    for ch in text.chars() {
        let Some(glyph) = font.glyphs.get(&ch) else {
            continue;
        };
        let width = glyph.iter().map(|r| row_chars(r).len()).max().unwrap_or(0);
        let mut overlap = width;
        for row in 0..font.height {
            let left = &buf[row];
            let right = row_chars(glyph.get(row).map(String::as_str).unwrap_or(""));
            let trail = left.iter().rev().take_while(|c| **c == ' ').count();
            let lead = right.iter().take_while(|c| **c == ' ').count();
            overlap = overlap.min(trail + lead);
        }
        let mut next: Vec<Vec<char>> = Vec::with_capacity(font.height);
        for row in 0..font.height {
            let left = &buf[row];
            let right = row_chars(glyph.get(row).map(String::as_str).unwrap_or(""));
            let keep = left.len().saturating_sub(overlap);
            let mut out: Vec<char> = left[..keep].to_vec();
            for i in 0..overlap {
                let idx = keep + i;
                let lc = left.get(idx).copied().unwrap_or(' ');
                let rc = right.get(i).copied().unwrap_or(' ');
                let merged = if lc == ' ' { rc } else { lc };
                if idx < left.len() {
                    out.push(merged);
                } else if merged != ' ' {
                    // Beyond the old row: kerning only ever puts spaces here.
                    out.push(merged);
                }
            }
            out.extend(right.iter().skip(overlap).copied());
            next.push(out);
        }
        buf = next;
    }
    buf.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|c| {
                    if c == font.hardblank || c == '\u{a0}' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn sm_width(text: &str) -> usize {
    compose_sm(text).iter().map(|r| r.chars().count()).max().unwrap_or(0)
}

/// Fold punctuation and whitespace for the block font. Returns `None` for
/// scripts FIGlet cannot draw.
fn figlet_text(text: &str) -> Option<String> {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            out.push(' ');
            continue;
        }
        let mapped = map_punct(ch);
        let parts: Vec<char> = match mapped {
            Some(value) => value.chars().collect(),
            None => vec![ch],
        };
        for part in parts {
            if !('\x20'..='\x7e').contains(&part) {
                return None;
            }
            out.push(part);
        }
    }
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Greedy FIGlet-style wrapping.
fn wrap_latin(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        // pyfiglet wraps a line that exactly fills the width.
        if sm_width(&candidate) < cols {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if sm_width(word) < cols {
            cur = word.to_string();
        } else {
            let mut piece = String::new();
            for ch in word.chars() {
                let candidate = format!("{piece}{ch}");
                if !piece.is_empty() && sm_width(&candidate) >= cols {
                    out.push(std::mem::take(&mut piece));
                }
                piece.push(ch);
            }
            cur = piece;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn paginate<T: Clone>(items: &[T], per_page: usize, progress: f64) -> Vec<T> {
    let per_page = per_page.max(1);
    if items.len() <= per_page {
        return items.to_vec();
    }
    let pages = items.len().div_ceil(per_page);
    let page = ((progress.clamp(0.0, 1.0) * pages as f64) as usize).min(pages - 1);
    let start = page * per_page;
    let end = (start + per_page).min(items.len());
    items[start..end].to_vec()
}

fn latin_lines(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in wrap_latin(text, cols) {
        out.extend(compose_sm(&chunk));
    }
    // Trim blank rows around the block.
    while out.first().is_some_and(|line| line.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|line| line.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Page through whole words. A word that cannot fit on its own means the
/// block font is unusable and the caller falls back to the bitmap font.
fn word_groups(text: &str, cols: usize, avail: usize) -> Option<Vec<String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut groups = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for word in words {
        let mut trial = cur.clone();
        trial.push(word);
        let joined = trial.join(" ");
        if latin_lines(&joined, cols).len() <= avail {
            cur.push(word);
            continue;
        }
        if cur.is_empty() {
            return None;
        }
        groups.push(cur.join(" "));
        cur.clear();
        if latin_lines(word, cols).len() > avail {
            return None;
        }
        cur.push(word);
    }
    if !cur.is_empty() {
        groups.push(cur.join(" "));
    }
    Some(groups)
}

fn render_latin(
    text: &str,
    cols: usize,
    avail: usize,
    progress: f64,
) -> Option<Vec<String>> {
    // The whole line may fit by height (pyfiglet can already wrap it).
    let lines = latin_lines(text, cols);
    if lines.len() <= avail {
        return Some(lines);
    }
    // Otherwise show one page of whole words at a time.
    let groups = word_groups(text, cols, avail)?;
    let page = ((progress.clamp(0.0, 1.0) * groups.len() as f64) as usize)
        .min(groups.len() - 1);
    Some(latin_lines(&groups[page], cols))
}

const MODES: [(&str, usize, usize, usize); 3] = [
    ("full", 1, 1, 1),
    ("half", 1, 2, 1),
    ("quad", 2, 2, 2),
];

fn chars_per_row(cols: usize, advance_px: usize, px_x: usize) -> usize {
    let cells_per_char = advance_px.div_ceil(px_x);
    (cols / cells_per_char.max(1)).max(1)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![String::new()];
    }
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let head: String = word.chars().take(width).collect();
            lines.push(head);
            word = word.chars().skip(width).collect();
        }
        let candidate = if cur.is_empty() {
            word.clone()
        } else {
            format!("{cur} {word}")
        };
        if candidate.chars().count() <= width {
            cur = candidate;
        } else {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur = word;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn map_punct(ch: char) -> Option<&'static str> {
    Some(match ch {
        '…' => "...",
        '–' | '—' | '―' | '−' | '•' | '·' => "-",
        '“' | '”' | '„' | '«' | '»' | '‟' => "\"",
        '‘' | '’' | '‚' | '‛' | '′' => "'",
        '″' => "\"",
        '№' => "N",
        '\u{a0}' | '\u{2009}' | '\u{202f}' => " ",
        '\u{200b}' => "",
        '≤' => "<",
        '≥' => ">",
        '→' => ">",
        '←' => "<",
        '×' => "X",
        _ => return None,
    })
}

fn fold_accent(ch: char) -> Option<&'static str> {
    Some(match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "A",
        'ç' => "C",
        'è' | 'é' | 'ê' | 'ë' => "E",
        'ì' | 'í' | 'î' | 'ï' => "I",
        'ñ' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "O",
        'ù' | 'ú' | 'û' | 'ü' => "U",
        'ý' | 'ÿ' => "Y",
        'æ' => "AE",
        'œ' => "OE",
        'ß' => "SS",
        _ => return None,
    })
}

fn normalize(text: &str) -> String {
    let font = bit();
    let mut out = String::new();
    let push = |ch: char, out: &mut String| {
        if ch == ' ' {
            out.push(' ');
        } else if font.glyphs.contains_key(&ch) {
            out.push(ch);
        } else {
            out.push('?');
        }
    };
    for raw in text.chars() {
        if raw.is_whitespace() {
            out.push(' ');
            continue;
        }
        if raw == 'ё' {
            out.push('Ё');
            continue;
        }
        if raw == 'й' {
            out.push('Й');
            continue;
        }
        if let Some(mapped) = map_punct(raw) {
            for ch in mapped.chars() {
                push(ch, &mut out);
            }
            continue;
        }
        if let Some(mapped) = fold_accent(raw) {
            for ch in mapped.chars() {
                push(ch, &mut out);
            }
            continue;
        }
        for ch in raw.to_uppercase() {
            push(ch, &mut out);
        }
    }
    out
}

fn text_bitmap(chunk: &str, gap: usize) -> Vec<String> {
    let font = bit();
    let glyphs: Vec<&Vec<String>> = chunk
        .chars()
        .filter_map(|ch| font.glyphs.get(&ch))
        .collect();
    if glyphs.is_empty() {
        return Vec::new();
    }
    let height = glyphs.iter().map(|g| g.len()).max().unwrap_or(0);
    let pad = ".".repeat(gap.max(1));
    (0..height)
        .map(|y| {
            let mut row = String::new();
            for (i, glyph) in glyphs.iter().enumerate() {
                if i > 0 {
                    row.push_str(&pad);
                }
                let top = height - glyph.len();
                if y < top {
                    row.push_str(".....");
                } else {
                    row.push_str(&glyph[y - top]);
                }
            }
            row
        })
        .collect()
}

fn cell_char(px_x: usize, px_y: usize, bits: usize) -> char {
    match (px_x, px_y) {
        (1, 1) => {
            if bits == 0 {
                ' '
            } else {
                '█'
            }
        }
        (1, 2) => [' ', '▀', '▄', '█'][bits.min(3)],
        _ => " ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█".chars().nth(bits.min(15)).unwrap_or(' '),
    }
}

fn bitmap_to_cells(bitmap: &[String], px_x: usize, px_y: usize) -> Vec<String> {
    if bitmap.is_empty() {
        return Vec::new();
    }
    let rows: Vec<Vec<char>> = bitmap.iter().map(|r| row_chars(r)).collect();
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut out = Vec::new();
    for y in (0..rows.len()).step_by(px_y) {
        let mut line = String::new();
        for x in (0..width).step_by(px_x) {
            let mut bits = 0;
            for dy in 0..px_y {
                for dx in 0..px_x {
                    let yy = y + dy;
                    let xx = x + dx;
                    if yy < rows.len() && xx < rows[yy].len() && rows[yy][xx] == '#' {
                        bits |= 1 << (dy * px_x + dx);
                    }
                }
            }
            line.push(cell_char(px_x, px_y, bits));
        }
        out.push(line.trim_end().to_string());
    }
    out
}

fn render_builtin(text: &str, cols: usize, avail: usize, progress: f64) -> Vec<String> {
    let norm = normalize(text);
    if norm.trim().is_empty() {
        return Vec::new();
    }

    let mut chosen: Option<(usize, usize, usize, Vec<String>)> = None;
    for (_, px_x, px_y, gap) in MODES {
        let cpl = chars_per_row(cols, 5 + gap, px_x);
        let chunks = wrap_text(&norm, cpl);
        let row_h = (8 + px_y - 1) / px_y;
        if chunks.len() == 1 && row_h <= avail {
            chosen = Some((px_x, px_y, gap, chunks));
            break;
        }
    }
    if chosen.is_none() {
        for (_, px_x, px_y, gap) in MODES {
            let cpl = chars_per_row(cols, 5 + gap, px_x);
            let chunks = wrap_text(&norm, cpl);
            let row_h = (8 + px_y - 1) / px_y;
            if chunks.len() * row_h <= avail {
                chosen = Some((px_x, px_y, gap, chunks));
                break;
            }
        }
    }

    let (px_x, px_y, gap, chunks) = chosen.unwrap_or_else(|| {
        let (_, px_x, px_y, gap) = MODES[1];
        let cpl = chars_per_row(cols, 5 + gap, px_x);
        (px_x, px_y, gap, wrap_text(&norm, cpl))
    });

    let row_h = (8 + px_y - 1) / px_y;
    let per_page = (avail / row_h).max(1);
    let page = paginate(&chunks, per_page, progress);

    let mut out = Vec::new();
    for chunk in page {
        let bitmap = text_bitmap(&chunk, gap);
        out.extend(bitmap_to_cells(&bitmap, px_x, px_y));
    }
    out
}

fn place(lines: &[String], cols: usize, avail: usize) -> Vec<String> {
    if lines.is_empty() || avail == 0 {
        return vec![String::new(); avail];
    }
    let lines: Vec<String> = lines
        .iter()
        .take(avail)
        .map(|line| {
            let line = line.trim_end();
            if line.chars().count() > cols {
                line.chars().take(cols).collect()
            } else {
                line.to_string()
            }
        })
        .collect();
    let height = lines.len();
    let free = avail.saturating_sub(height);
    let top = (free + 1) / 2;
    let bottom = free - top;
    let visible = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let left = cols.saturating_sub(visible) / 2;

    let mut out = vec![String::new(); top];
    for line in &lines {
        let mut row = " ".repeat(left);
        row.push_str(line);
        out.push(row);
    }
    out.extend(vec![String::new(); bottom]);
    out.truncate(avail);
    out
}

/// Side padding grows with the terminal width.
pub fn side_margin(cols: usize) -> usize {
    if cols >= 110 {
        8
    } else if cols >= 90 {
        6
    } else if cols >= 70 {
        5
    } else if cols >= 55 {
        4
    } else if cols >= 40 {
        3
    } else if cols >= 28 {
        2
    } else {
        1
    }
}

/// Tighter padding for the full-screen player; `--banner` keeps the original
/// margins.
pub fn compact_side_margin(cols: usize) -> usize {
    if cols >= 100 {
        4
    } else if cols >= 80 {
        3
    } else if cols >= 60 {
        2
    } else if cols >= 40 {
        1
    } else {
        0
    }
}

/// Reserve a blank row above and below when possible.
fn banner_max_height(avail: usize) -> usize {
    if avail >= 6 {
        (avail - 2).max(1)
    } else {
        avail
    }
}

fn max_width(lines: &[String]) -> usize {
    lines.iter().map(|line| line.chars().count()).max().unwrap_or(0)
}

/// Render with either font. The flag says whether the block font was used.
fn render_banner(
    text: &str,
    cols: usize,
    avail: usize,
    progress: f64,
) -> (Vec<String>, bool) {
    match figlet_text(text) {
        Some(prepared) => match render_latin(&prepared, cols, avail, progress) {
            Some(lines) => (lines, true),
            None => (render_builtin(text, cols, avail, progress), false),
        },
        None => (render_builtin(text, cols, avail, progress), false),
    }
}

/// Fit inside the margins, borrowing width or height only to keep the block
/// typeface.
fn fit_banner(
    text: &str,
    cols: usize,
    avail: usize,
    progress: f64,
    compact: bool,
) -> (Vec<String>, bool) {
    let side = if compact {
        compact_side_margin(cols)
    } else {
        side_margin(cols)
    };
    let inner = (cols.saturating_sub(2 * side)).max(10);
    let height = if compact {
        avail
    } else {
        banner_max_height(avail)
    };
    let (mut lines, mut figlet) = render_banner(text, inner, height, progress);
    let wants_figlet = figlet_text(text).is_some();
    let wide_cols = cols.saturating_sub(1).max(10);

    if !lines.is_empty() && max_width(&lines) > inner {
        let (wide, wide_figlet) = render_banner(text, wide_cols, height, progress);
        if !wide.is_empty() {
            lines = wide;
            figlet = wide_figlet;
        }
    } else if wants_figlet && !figlet {
        let (wide, wide_figlet) = render_banner(text, wide_cols, height, progress);
        if wide_figlet {
            lines = wide;
            figlet = wide_figlet;
        }
    }
    if lines.is_empty() && height != avail {
        (lines, figlet) = render_banner(text, inner, avail, progress);
    }
    if lines.is_empty() && inner < wide_cols {
        (lines, figlet) = render_banner(text, wide_cols, avail, progress);
    }
    if wants_figlet && !figlet && wide_cols < cols {
        let (full, full_figlet) = render_banner(text, cols, avail, progress);
        if full_figlet {
            lines = full;
            figlet = full_figlet;
        }
    }
    (lines, figlet)
}

/// Split `(parenthetical)` text from the main line. The notes keep their
/// brackets and are rendered on separate lines below the main text.
fn split_parenthetical(text: &str) -> (String, Vec<String>) {
    let mut main = String::new();
    let mut notes = Vec::new();
    let mut depth = 0usize;
    let mut note = String::new();
    for ch in text.chars() {
        match ch {
            '(' => {
                if depth == 0 {
                    depth = 1;
                    note.clear();
                } else {
                    depth += 1;
                    note.push(ch);
                }
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let content = note.trim();
                    if !content.is_empty() {
                        notes.push(format!("({content})"));
                    }
                    note.clear();
                } else {
                    note.push(ch);
                }
            }
            _ => {
                if depth > 0 {
                    note.push(ch);
                } else {
                    main.push(ch);
                }
            }
        }
    }
    if depth > 0 {
        // An unclosed bracket is ordinary text.
        main.push('(');
        main.push_str(&note);
    }
    let main = main.split_whitespace().collect::<Vec<_>>().join(" ");
    (main, notes)
}

fn render_parts(
    text: &str,
    cols: usize,
    avail: usize,
    progress: f64,
    compact: bool,
) -> Vec<String> {
    let (main, notes) = split_parenthetical(text);
    if notes.is_empty() {
        let (lines, _figlet) = fit_banner(text, cols, avail, progress, compact);
        return place(&lines, cols, avail);
    }

    let mut parts = Vec::new();
    if !main.is_empty() {
        parts.push(main);
    }
    parts.extend(notes);

    let total = parts.len();
    let mut lines = Vec::new();
    let mut remaining = avail;
    for (index, part) in parts.iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let height = (remaining / (total - index)).max(1);
        let (mut part_lines, _figlet) =
            fit_banner(part, cols, height, progress, compact);
        part_lines.truncate(height);
        remaining = remaining.saturating_sub(part_lines.len());
        lines.extend(part_lines);
    }
    place(&lines, cols, avail)
}

/// Render a lyric line as a block banner.
pub fn render(text: &str, cols: usize, avail: usize, progress: f64) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() || cols == 0 || avail == 0 {
        return vec![String::new(); avail];
    }
    render_parts(text, cols, avail, progress, false)
}

/// Compact variant for the TUI: less padding, full body height.
pub fn render_compact(text: &str, cols: usize, avail: usize, progress: f64) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() || cols == 0 || avail == 0 {
        return vec![String::new(); avail];
    }
    render_parts(text, cols, avail, progress, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_never_exceeds_the_screen() {
        for cols in [16, 28, 40, 60, 80, 100] {
            for text in ["The line changes with the music", "When you were here before"] {
                let lines = render(text, cols, 20, 0.0);
                assert_eq!(lines.len(), 20);
                assert!(lines.iter().all(|line| line.chars().count() <= cols));
            }
        }
    }

    #[test]
    fn latin_text_uses_the_block_font() {
        let lines = render("HELLO", 80, 20, 0.0);
        assert!(lines.iter().any(|line| line.contains('▀') || line.contains('▌')));
    }

    #[test]
    fn cyrillic_text_uses_the_bitmap_font() {
        let lines = render("Привет", 80, 20, 0.0);
        assert!(lines.iter().any(|line| line.contains('█')));
    }

    #[test]
    fn square_brackets_are_separated_from_the_text() {
        let lines = compose_sm("[o]");
        assert!(lines.iter().any(|line| line.contains("▝▀▘ ▝▀")));
        assert!(lines.iter().all(|line| !line.contains('\u{a0}')));
    }

    #[test]
    fn parenthetical_text_is_split_from_the_main_line() {
        let (main, notes) = split_parenthetical("Hello (world) again (twice)");
        assert_eq!(main, "Hello again");
        assert_eq!(notes, vec!["(world)".to_string(), "(twice)".to_string()]);
        let (main, notes) = split_parenthetical("(only a note)");
        assert!(main.is_empty());
        assert_eq!(notes, vec!["(only a note)".to_string()]);
    }

    #[test]
    fn parenthetical_banner_stays_inside_the_screen() {
        let lines = render_compact("Main line (wrapped onto another line)", 80, 20, 0.0);
        assert_eq!(lines.len(), 20);
        assert!(lines.iter().all(|line| line.chars().count() <= 80));
    }
}
