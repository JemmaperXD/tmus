use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};
use ratatui::Frame;

use crate::app::{
    App, BuilderFocus, Input, LibraryFocus, LyricState, Row, RowKind, TagField, View,
};
use crate::keys::{self, Action};
use crate::util;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if app.show_lyrics {
        draw_lyrics(frame, app, area);
    } else {
        draw_main(frame, app, area);
    }

    if app.tag_editor.is_some() {
        draw_tag_editor(frame, app, area);
    } else if app.builder.is_some() {
        draw_builder(frame, app, area);
    } else if app.keymap_editor.is_some() {
        draw_keymap_editor(frame, app, area);
    } else if app.help.is_some() {
        draw_help(frame, app, area);
    }
}

fn draw_main(frame: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    draw_tabs(frame, app, chunks[0]);
    if app.view == View::Library {
        draw_library(frame, app, chunks[1]);
    } else {
        draw_list(frame, app, chunks[1]);
    }
    draw_status(frame, app, chunks[2]);
    draw_prompt(frame, app, chunks[3]);
}

pub const ACCENT: Color = Color::Blue;

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let key = |index: usize, view: View| -> String {
        app.keymap
            .keys(view.action())
            .first()
            .map(|binding| binding.display())
            .unwrap_or_else(|| (index + 1).to_string())
    };
    let full: Vec<Line<'static>> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| Line::from(format!(" {} {} ", key(index, *view), view.title())))
        .collect();
    let short: Vec<Line<'static>> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| Line::from(format!(" {} {} ", key(index, *view), view.short_title())))
        .collect();
    let numbers: Vec<Line<'static>> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| Line::from(format!(" {} ", key(index, *view))))
        .collect();
    let tab_width = |titles: &[Line<'static>]| -> usize {
        titles
            .iter()
            .map(|line| line.width())
            .sum::<usize>()
    };
    let titles = if tab_width(&full) <= width {
        full
    } else if tab_width(&short) <= width {
        short
    } else {
        numbers
    };
    let tabs = Tabs::new(titles)
        .select(app.view.idx())
        .style(Style::new().bg(ACCENT).fg(Color::Gray))
        .highlight_style(
            Style::new()
                .bg(ACCENT)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .divider("")
        .padding("", "");
    frame.render_widget(Block::default().style(Style::new().bg(ACCENT)), area);
    frame.render_widget(tabs, area);
}

fn row_item(app: &App, row: &Row, width: usize) -> ListItem<'static> {
    let marked = row
        .track
        .as_ref()
        .map(|track| app.marks.contains(&track.path))
        .unwrap_or(false);
    let prefix = if marked { "• " } else { "  " };
    let indent = "  ".repeat(row.indent);
    if row.kind == RowKind::Album {
        // Album rows are separators in the track column.
        let head = format!("── {} ", row.text);
        let fill = width.saturating_sub(util::char_len(&head));
        let text = format!("{head}{}", "─".repeat(fill));
        return ListItem::new(text).style(
            Style::new()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    }
    let text = format!("{prefix}{indent}{}", row.text);
    let style = match row.kind {
        RowKind::Album => Style::new().fg(Color::Yellow),
        RowKind::Track => Style::new().fg(Color::White),
        RowKind::Dir => Style::new()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD),
        RowKind::File => Style::new().fg(Color::White),
        RowKind::Parent => Style::new().fg(Color::Gray),
        RowKind::Setting => Style::new().fg(Color::LightCyan),
    };
    ListItem::new(text).style(style)
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    app.list_height = area.height.max(1) as usize;
    let width = area.width as usize;
    let items: Vec<ListItem<'static>> = app
        .rows
        .iter()
        .map(|row| row_item(app, row, width))
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::new()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    let state = &mut app.states[app.view.idx()];
    frame.render_stateful_widget(list, area, state);
}

/// Library: artists on the left, tracks with album separators on the right.
fn draw_library(frame: &mut Frame, app: &mut App, area: Rect) {
    app.list_height = area.height.max(1) as usize;
    let columns = Layout::horizontal([
        Constraint::Percentage(33),
        Constraint::Length(1),
        Constraint::Min(10),
    ])
    .split(area);
    let (left, divider, right) = (columns[0], columns[1], columns[2]);

    let focused_artists = app.library_focus == LibraryFocus::Artists;
    let current_artist = app
        .current
        .as_ref()
        .map(|track| track.artist_or().to_lowercase());
    let items: Vec<ListItem<'static>> = app
        .library_artists
        .iter()
        .map(|artist| {
            let playing = current_artist
                .as_deref()
                .is_some_and(|name| name == artist.name.to_lowercase());
            let text = if playing {
                format!("▶ {}", artist.name)
            } else {
                format!("  {}", artist.name)
            };
            let style = if playing {
                Style::new().fg(Color::LightGreen)
            } else {
                Style::new()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            };
            ListItem::new(text).style(style)
        })
        .collect();
    let highlight = if focused_artists {
        Style::new()
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(Color::DarkGray).fg(Color::White)
    };
    let list = List::new(items)
        .highlight_style(highlight)
        .highlight_symbol("");
    frame.render_stateful_widget(list, left, &mut app.library_artist_state);

    let divider_line = "│".repeat(divider.height as usize);
    frame.render_widget(
        Paragraph::new(divider_line).style(Style::new().fg(Color::DarkGray)),
        divider,
    );

    let width = right.width as usize;
    let items: Vec<ListItem<'static>> = app
        .rows
        .iter()
        .map(|row| row_item(app, row, width))
        .collect();
    let highlight = if focused_artists {
        Style::new().bg(Color::DarkGray).fg(Color::White)
    } else {
        Style::new()
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    };
    let list = List::new(items)
        .highlight_style(highlight)
        .highlight_symbol("");
    frame.render_stateful_widget(list, right, &mut app.states[View::Library.idx()]);
}

fn clock(app: &App) -> String {
    if app.remaining && app.duration > 0.0 {
        format!("-{}", util::fmt_time((app.duration - app.position).max(0.0)))
    } else {
        util::fmt_time(app.position)
    }
}

fn two_sided(left: &str, right: &str, width: usize) -> Line<'static> {
    let right_len = util::char_len(right);
    let left = util::truncate(left, width.saturating_sub(right_len + 1));
    let pad = width.saturating_sub(util::char_len(&left) + right_len);
    Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::raw(right.to_string()),
    ])
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let left = app
        .current
        .as_ref()
        .map(|track| track.label())
        .unwrap_or_else(|| "tmus".to_string());

    let mut flags = Vec::new();
    if app.repeat {
        flags.push("repeat");
    }
    if app.repeat_current {
        flags.push("repeat 1");
    }
    if app.shuffle {
        flags.push("shuffle");
    }
    if app.continue_play {
        flags.push("continue");
    }
    if app.aaa_mode {
        flags.push("album");
    }
    if app.play_library {
        flags.push("library");
    }
    if app.follow {
        flags.push("follow");
    }
    if app.scanning {
        flags.push("scan");
    }

    let time = format!(
        "{}{} / {}",
        if app.paused { "‖ " } else { "" },
        clock(app),
        util::fmt_time(app.duration)
    );
    // Drop flags that no longer fit rather than push the clock off-screen.
    let mut flags_text = String::new();
    for flag in &flags {
        let candidate = if flags_text.is_empty() {
            (*flag).to_string()
        } else {
            format!("{flags_text} {flag}")
        };
        if util::char_len(&candidate) + util::char_len(&time) + 12 <= width {
            flags_text = candidate;
        }
    }
    let mut right = String::new();
    if !flags_text.is_empty() {
        right.push_str(&flags_text);
        right.push_str("  ");
    }
    right.push_str(&time);

    let line = two_sided(&left, &right, width);
    frame.render_widget(
        Paragraph::new(line).style(Style::new().bg(Color::Gray).fg(Color::Black)),
        area,
    );
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let (text, style) = match app.input {
        Input::Command => (format!(":{}", app.buf), Style::new().fg(Color::White)),
        Input::Search => (format!("/{}", app.buf), Style::new().fg(Color::White)),
        Input::DeleteFile => {
            let path = app
                .pending_delete
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            (
                format!("Delete file {path}? y/n"),
                Style::new().fg(Color::LightRed),
            )
        }
        Input::Quit => (app.message.clone(), Style::new().fg(Color::Yellow)),
        Input::None => {
            if !app.message.is_empty() {
                (app.message.clone(), Style::new().fg(Color::Yellow))
            } else {
                (
                    "l — lyrics   : — command   / — search   q — quit".to_string(),
                    Style::new().fg(Color::DarkGray),
                )
            }
        }
    };
    let prompt_len = match app.input {
        Input::Command | Input::Search => 1,
        _ => 0,
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
    if prompt_len > 0 {
        let x = area
            .x
            .saturating_add((prompt_len + app.buf.chars().count()) as u16)
            .min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(x, area.y));
    }
}

const CAVA_COLOR: Color = Color::Indexed(252);

fn draw_lyrics(frame: &mut Frame, app: &mut App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::new().bg(Color::Reset).fg(Color::Reset)),
        area,
    );
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);

    draw_lyric_header(frame, app, chunks[0]);

    let inner = chunks[1];
    let width = inner.width as usize;
    let height = inner.height as usize;
    let values = app.cava_values();
    let back = if app.cava_enabled && !values.is_empty() {
        crate::cava::canvas(&values, height, width)
    } else {
        vec![" ".repeat(width); height]
    };

    let ready = app.lyric_state == LyricState::Ready && app.lyrics.is_some();
    let body = if app.lyrics_hidden {
        vec![String::new(); height]
    } else if ready {
        app.banner_lines(width, height)
    } else {
        lyric_message(app, width, height)
    };
    let text_style = if ready {
        Style::new()
            .fg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let lines = paint_scene(&body, &back, width, text_style);
    frame.render_widget(Paragraph::new(lines), inner);

    if app.input != Input::None {
        draw_prompt(frame, app, chunks[2]);
    } else {
        draw_lyric_footer(frame, app, chunks[2]);
    }
}

fn draw_lyric_header(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let left = app
        .current
        .as_ref()
        .map(|track| format!("●  {} — {}", track.artist_or(), track.title_or()))
        .unwrap_or_else(|| "●  tmus".to_string());
    let right = format!(
        "{}{} / {}",
        if app.paused { "‖ " } else { "" },
        util::fmt_time(app.position),
        util::fmt_time(app.duration)
    );
    let room = width.saturating_sub(util::char_len(&right) + 2);
    let left = util::truncate(&left, room);
    let pad = width
        .saturating_sub(util::char_len(&left) + util::char_len(&right))
        .max(1);
    let plain = format!("{left}{}{right}", " ".repeat(pad));
    let plain = util::truncate(&plain, width);
    let mut chars = plain.chars();
    let dot = chars.next().unwrap_or(' ');
    let rest: String = chars.collect();
    let line = Line::from(vec![
        Span::styled(dot.to_string(), Style::new().fg(ACCENT)),
        Span::styled(rest, Style::new().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_lyric_footer(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let barw = 5.max(40.min(width.saturating_sub(18)));
    let frac = if app.duration > 0.0 {
        (app.position / app.duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (frac * barw as f64).round() as usize;
    let bar = format!("{}{}", "━".repeat(filled), "─".repeat(barw - filled));
    let left = util::fmt_time(app.position);
    let right = util::fmt_time(app.duration);
    let base_len = 1 + util::char_len(&left) + 1 + barw + 1 + util::char_len(&right);
    if base_len > width {
        // Narrow terminals cut the footer instead of reflowing it.
        let line = util::truncate(&format!(" {left} {bar} {right}"), width);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                line,
                Style::new().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let mut note = if app.message.is_empty() {
        app.lyric_note()
    } else {
        app.message.clone()
    };
    let room = width.saturating_sub(base_len + 4);
    if room >= 4 {
        note = util::truncate(&note, room);
    } else {
        note.clear();
    }
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(left, Style::new().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled(bar, Style::new().fg(ACCENT)),
        Span::raw(" "),
        Span::styled(right, Style::new().fg(Color::DarkGray)),
    ];
    if !note.is_empty() {
        spans.push(Span::styled(
            format!(" · {note}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn lyric_message(app: &App, width: usize, height: usize) -> Vec<String> {
    let message = if app.current.is_none() {
        "no active track — start playing music".to_string()
    } else {
        match app.lyric_state {
            LyricState::Loading => {
                let waited = app.lyric_wait();
                if waited < 1 {
                    "searching for lyrics on lrclib.net…".to_string()
                } else {
                    format!("searching for lyrics on lrclib.net… {waited} s")
                }
            }
            LyricState::Missing => {
                let reason = app.lyric_error.clone();
                match app.lyric_retry_in() {
                    Some(remain) => format!(
                        "{reason}\nretrying in {remain} s · r — try now"
                    ),
                    None => format!("{reason}\n r — try now"),
                }
            }
            _ => "no lyrics".to_string(),
        }
    };
    let side = crate::banner::compact_side_margin(width);
    let inner_width = width.saturating_sub(2 * side).max(16);
    let mut lines = Vec::new();
    for part in message.lines() {
        lines.extend(wrap_plain(part, inner_width));
    }
    place_center(&lines, width, height)
}

fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while util::char_len(&word) > width {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let head: String = word.chars().take(width).collect();
            out.push(head);
            word = word.chars().skip(width).collect();
        }
        let candidate = if cur.is_empty() {
            word.clone()
        } else {
            format!("{cur} {word}")
        };
        if util::char_len(&candidate) <= width {
            cur = candidate;
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur = word;
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

fn place_center(lines: &[String], width: usize, height: usize) -> Vec<String> {
    // Common left margin from the longest line; an extra blank row when the
    // free space is odd.
    let lines: Vec<String> = lines
        .iter()
        .take(height)
        .map(|line| util::truncate(line, width))
        .collect();
    let visible = lines
        .iter()
        .map(|line| util::char_len(line))
        .max()
        .unwrap_or(0);
    let left = width.saturating_sub(visible) / 2;
    let free = height.saturating_sub(lines.len());
    let top = (free + 1) / 2;
    let mut out = vec![String::new(); top];
    for line in lines {
        out.push(format!("{}{}", " ".repeat(left), line));
    }
    out.extend(vec![String::new(); height.saturating_sub(out.len())]);
    out
}

/// Paint the banner over the visualizer. A glyph wins, but its cell keeps the
/// bar colour as background so the bar stays continuous.
fn paint_scene(
    text: &[String],
    back: &[String],
    width: usize,
    text_style: Style,
) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(text.len());
    for (y, line) in text.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let back_row: Vec<char> = back
            .get(y)
            .map(|row| row.chars().collect())
            .unwrap_or_default();
        let mut spans: Vec<(Style, String)> = Vec::new();
        for x in 0..width {
            let ch = chars.get(x).copied().unwrap_or(' ');
            let on_bar = back_row.get(x).is_some_and(|cell| *cell != ' ');
            let (cell, style) = if ch != ' ' {
                let style = if on_bar {
                    text_style.bg(CAVA_COLOR)
                } else {
                    text_style
                };
                (ch, style)
            } else if on_bar {
                ('█', Style::new().fg(CAVA_COLOR))
            } else {
                (' ', Style::default())
            };
            match spans.last_mut() {
                Some((last_style, text)) if *last_style == style => text.push(cell),
                _ => spans.push((style, cell.to_string())),
            }
        }
        out.push(Line::from(
            spans
                .into_iter()
                .map(|(style, text)| Span::styled(text, style))
                .collect::<Vec<_>>(),
        ));
    }
    out
}

fn popup_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn builder_item(track: &crate::library::Track) -> ListItem<'static> {
    ListItem::new(format!("{}  ·  {}", track.artist_or(), track.list_text()))
}

fn draw_tag_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    const LABEL_WIDTH: usize = 20;

    let Some(editor) = app.tag_editor.as_mut() else {
        return;
    };
    let popup = popup_rect(
        area,
        area.width.saturating_sub(6).clamp(46, 96),
        area.height.saturating_sub(2).clamp(16, 30),
    );
    frame.render_widget(Clear, popup);

    let title = if editor.lyrics_mode {
        " Lyrics · Esc to fields · Ctrl+S save "
    } else {
        " Tags · Ctrl+S save · Esc close "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::new().bg(Color::Black).fg(Color::White));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(2),
    ])
    .split(inner);

    let path = format!("File: {}", editor.path.display());
    frame.render_widget(
        Paragraph::new(util::truncate(&path, chunks[0].width as usize))
            .style(Style::new().fg(Color::Gray)),
        chunks[0],
    );

    if editor.lyrics_mode {
        draw_lyrics_editor(frame, editor, chunks[1]);
    } else {
        draw_tag_fields(frame, editor, chunks[1]);
    }

    let message = if editor.message.is_empty() {
        editor.track.clone()
    } else {
        editor.message.clone()
    };
    frame.render_widget(
        Paragraph::new(util::truncate(&message, chunks[2].width as usize)).style(
            if editor.message.is_empty() {
                Style::new().fg(Color::DarkGray)
            } else {
                Style::new().fg(Color::Yellow)
            },
        ),
        Rect {
            x: chunks[2].x,
            y: chunks[2].y,
            width: chunks[2].width,
            height: 1,
        },
    );
    let hint = if editor.lyrics_mode {
        "Enter — new line · arrows — cursor · Ctrl+S — save"
    } else {
        "↑/↓ — field · Enter — next · Ctrl+S — save · Esc — close"
    };
    frame.render_widget(
        Paragraph::new(util::truncate(hint, chunks[2].width as usize))
            .style(Style::new().fg(Color::Gray)),
        Rect {
            x: chunks[2].x,
            y: chunks[2].y + 1,
            width: chunks[2].width,
            height: 1,
        },
    );

    if !editor.lyrics_mode {
        let field = editor.selected();
        let index = editor.field as u16;
        if field != TagField::Lyrics && index < chunks[1].height {
            let value_width = (chunks[1].width as usize).saturating_sub(LABEL_WIDTH);
            if value_width > 0 {
                let value = editor.value(field);
                let cursor = editor.cursor.min(value.len());
                let column = value.get(..cursor).unwrap_or("").chars().count();
                let scroll = column.saturating_sub(value_width.saturating_sub(1));
                let x = chunks[1].x
                    + LABEL_WIDTH as u16
                    + (column - scroll) as u16;
                let x = x.min(chunks[1].x + chunks[1].width.saturating_sub(1));
                frame.set_cursor_position(Position::new(x, chunks[1].y + index));
            }
        }
    }
}

fn draw_tag_fields(frame: &mut Frame, editor: &crate::app::TagEditor, area: Rect) {
    const LABEL_WIDTH: usize = 20;

    let value_width = (area.width as usize).saturating_sub(LABEL_WIDTH);
    let items: Vec<ListItem<'static>> = TagField::ALL
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == editor.field;
            let raw = editor.value(*field);
            let value = if *field == TagField::Lyrics {
                if raw.trim().is_empty() {
                    "empty".to_string()
                } else {
                    format!("{} lines", editor.lyrics_line_count())
                }
            } else if selected {
                let cursor = editor.cursor.min(raw.len());
                let column = raw.get(..cursor).unwrap_or("").chars().count();
                let scroll = column.saturating_sub(value_width.saturating_sub(1));
                raw.chars().skip(scroll).take(value_width).collect()
            } else {
                util::truncate(raw, value_width)
            };
            let label_style = if selected {
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::LightCyan)
            };
            let value_style = if selected {
                Style::new().fg(Color::Black).bg(Color::White)
            } else if raw.is_empty() {
                Style::new().fg(Color::DarkGray)
            } else {
                Style::new().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", field.label(), width = LABEL_WIDTH),
                    label_style,
                ),
                Span::styled(value, value_style),
            ]))
        })
        .collect();
    frame.render_widget(List::new(items), area);
}

fn draw_lyrics_editor(
    frame: &mut Frame,
    editor: &mut crate::app::TagEditor,
    area: Rect,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }

    let (line, column) = editor.lyrics_position();
    if line < editor.lyrics_scroll {
        editor.lyrics_scroll = line;
    }
    if line >= editor.lyrics_scroll + height {
        editor.lyrics_scroll = line + 1 - height;
    }
    if column < editor.lyrics_hscroll {
        editor.lyrics_hscroll = column;
    }
    if column >= editor.lyrics_hscroll + width {
        editor.lyrics_hscroll = column + 1 - width;
    }

    let lines: Vec<Line<'static>> = editor
        .data
        .lyrics
        .split('\n')
        .skip(editor.lyrics_scroll)
        .take(height)
        .map(|line| {
            let text: String = line
                .chars()
                .skip(editor.lyrics_hscroll)
                .take(width)
                .collect();
            Line::from(Span::styled(text, Style::new().fg(Color::White)))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);

    let x = area.x + (column - editor.lyrics_hscroll) as u16;
    let y = area.y + (line - editor.lyrics_scroll) as u16;
    let x = x.min(area.x + area.width.saturating_sub(1));
    let y = y.min(area.y + area.height.saturating_sub(1));
    frame.set_cursor_position(Position::new(x, y));
}

fn draw_builder(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(builder) = app.builder.as_mut() else {
        return;
    };
    let popup = popup_rect(
        area,
        area.width.saturating_sub(4).clamp(24, 120),
        area.height.saturating_sub(2).clamp(10, 34),
    );
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Playlist builder · Ctrl+S save · Esc close ")
        .style(Style::new().bg(Color::Black).fg(Color::White));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(inner);

    let name_focus = builder.focus == BuilderFocus::Name;
    let name_text = if builder.name.is_empty() {
        "enter a name".to_string()
    } else {
        builder.name.clone()
    };
    let name_style = if builder.name.is_empty() {
        Style::new().fg(Color::DarkGray)
    } else if name_focus {
        Style::new()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::White)
    };
    let name_block = Block::default()
        .borders(Borders::ALL)
        .title(" Name ")
        .border_style(if name_focus {
            Style::new().fg(Color::LightCyan)
        } else {
            Style::new().fg(Color::DarkGray)
        });
    frame.render_widget(
        Paragraph::new(format!(" {name_text}"))
            .style(name_style)
            .block(name_block),
        chunks[0],
    );

    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let left_focus = builder.focus == BuilderFocus::Left;
    let left_items: Vec<ListItem<'static>> =
        builder.left.iter().map(builder_item).collect();
    let left_list = List::new(left_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Library · {} ", builder.left.len()))
                .border_style(if left_focus {
                    Style::new().fg(Color::LightCyan)
                } else {
                    Style::new().fg(Color::DarkGray)
                }),
        )
        .highlight_style(if left_focus {
            Style::new()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(Color::DarkGray).fg(Color::White)
        })
        .highlight_symbol("");
    frame.render_stateful_widget(left_list, columns[0], &mut builder.left_state);

    let right_focus = builder.focus == BuilderFocus::Right;
    let right_items: Vec<ListItem<'static>> =
        builder.right.iter().map(builder_item).collect();
    let right_list = List::new(right_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Playlist · {} ", builder.right.len()))
                .border_style(if right_focus {
                    Style::new().fg(Color::LightCyan)
                } else {
                    Style::new().fg(Color::DarkGray)
                }),
        )
        .highlight_style(if right_focus {
            Style::new()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().bg(Color::DarkGray).fg(Color::White)
        })
        .highlight_symbol("");
    frame.render_stateful_widget(right_list, columns[1], &mut builder.right_state);
}

fn draw_keymap_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let popup = popup_rect(
        area,
        area.width.saturating_sub(4).clamp(40, 110),
        area.height.saturating_sub(2).clamp(10, 34),
    );
    frame.render_widget(Clear, popup);

    let items: Vec<ListItem<'static>> = Action::ALL
        .iter()
        .map(|action| {
            ListItem::new(format!(
                "{:<30} {}",
                action.label(),
                app.keymap.display(*action)
            ))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Keys ")
        .style(Style::new().bg(Color::Black).fg(Color::White));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);

    let Some(editor) = app.keymap_editor.as_mut() else {
        return;
    };
    let list = List::new(items)
        .highlight_style(
            Style::new()
                .bg(Color::White)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    frame.render_stateful_widget(list, chunks[0], &mut editor.state);

    let hint = if editor.capture {
        editor.message.clone()
    } else if editor.message.is_empty() {
        "Enter — change · r — reset · Esc — close".to_string()
    } else {
        format!(
            "{} · Enter — change · r — reset · Esc — close",
            editor.message
        )
    };
    frame.render_widget(
        Paragraph::new(hint).style(if editor.capture {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new().fg(Color::Gray)
        }),
        chunks[1],
    );
}

fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let popup = popup_rect(
        area,
        area.width.saturating_sub(4).clamp(40, 110),
        area.height.saturating_sub(2).clamp(10, 40),
    );
    frame.render_widget(Clear, popup);

    let items: Vec<ListItem<'static>> = keys::rows(&app.keymap)
        .into_iter()
        .map(|row| {
            if row.header {
                ListItem::new(Line::from(Span::styled(
                    row.label,
                    Style::new()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("  {:<18}", row.key),
                        Style::new().fg(Color::LightCyan),
                    ),
                    Span::raw(row.label),
                ]))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help · Esc close · ↑/↓ scroll ")
        .style(Style::new().bg(Color::Black).fg(Color::White));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("");
    let Some(help) = app.help.as_mut() else {
        return;
    };
    frame.render_stateful_widget(list, popup, &mut help.state);
}
