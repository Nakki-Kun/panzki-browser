use std::io::Stdout;
use std::sync::Once;

use anyhow::Result;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use crate::app::{App, Mode, Overlay, StatusTone};
use crate::render::{RLine, SpanStyle, Tone};
use crate::util;

// ---------------------------------------------------------------------------
// Terminal lifecycle
// ---------------------------------------------------------------------------

static PANIC_HOOK: Once = Once::new();

pub fn init() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    PANIC_HOOK.call_once(|| {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(
                std::io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                DisableBracketedPaste,
                Show
            );
            hook(info);
        }));
    });

    enable_raw_mode()?;
    execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(std::io::stdout());
    Ok(Terminal::new(backend)?)
}

pub fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Palette {
    text: Color,
    dim: Color,
    accent: Color,
    link: Color,
    code: Color,
    danger: Color,
    success: Color,
    heading: Color,
    quote: Color,
    border: Color,
    sel_bg: Color,
    sel_fg: Color,
    bar_bg: Color,
    bar_fg: Color,
    tab_active_bg: Color,
    tab_active_fg: Color,
}

fn palette(light: bool) -> Palette {
    if light {
        Palette {
            text: Color::Black,
            dim: Color::DarkGray,
            accent: Color::Blue,
            link: Color::Blue,
            code: Color::Magenta,
            danger: Color::Red,
            success: Color::Green,
            heading: Color::Blue,
            quote: Color::DarkGray,
            border: Color::Gray,
            sel_bg: Color::LightBlue,
            sel_fg: Color::Black,
            bar_bg: Color::Gray,
            bar_fg: Color::Black,
            tab_active_bg: Color::Blue,
            tab_active_fg: Color::White,
        }
    } else {
        Palette {
            text: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Cyan,
            link: Color::LightBlue,
            code: Color::Yellow,
            danger: Color::LightRed,
            success: Color::LightGreen,
            heading: Color::LightCyan,
            quote: Color::Gray,
            border: Color::DarkGray,
            sel_bg: Color::Blue,
            sel_fg: Color::White,
            bar_bg: Color::DarkGray,
            bar_fg: Color::White,
            tab_active_bg: Color::Cyan,
            tab_active_fg: Color::Black,
        }
    }
}

fn tone_color(tone: Tone, p: &Palette) -> Color {
    match tone {
        Tone::Text => p.text,
        Tone::Dim => p.dim,
        Tone::Accent => p.accent,
        Tone::Link => p.link,
        Tone::Code => p.code,
        Tone::Heading => p.heading,
        Tone::Quote => p.quote,
    }
}

fn span_style(style: SpanStyle, p: &Palette) -> Style {
    let mut modifiers = Modifier::empty();
    if style.bold {
        modifiers |= Modifier::BOLD;
    }
    if style.italic {
        modifiers |= Modifier::ITALIC;
    }
    if style.underline {
        modifiers |= Modifier::UNDERLINED;
    }
    if style.strike {
        modifiers |= Modifier::CROSSED_OUT;
    }
    Style::default()
        .fg(tone_color(style.tone, p))
        .add_modifier(modifiers)
}

// ---------------------------------------------------------------------------
// Main draw
// ---------------------------------------------------------------------------

pub fn draw(frame: &mut Frame, app: &mut App) {
    let p = palette(app.store.config.light());
    let area = frame.area();
    if app.visual.is_some() {
        draw_visual(frame, area, app, &p);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_tabs(frame, chunks[0], app, &p);
    draw_address(frame, chunks[1], app, &p);
    draw_content(frame, chunks[2], app, &p);
    draw_status(frame, chunks[3], app, &p);
    draw_hints(frame, chunks[4], app, &p);

    if app.overlay.is_some() {
        frame.render_widget(Clear, chunks[2]);
        draw_overlay(frame, area, app, &p);
    }
}

fn draw_visual(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let Some(visual) = app.visual.as_ref() else {
        return;
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let header = format!(
        " VISUAL  {}  {}x{}  {}/{} ",
        util::truncate(&visual.url, area.width.saturating_sub(30) as usize),
        visual.width,
        visual.height,
        (visual.scroll + 1).min(visual.height),
        visual.height
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            header,
            Style::default()
                .bg(p.accent)
                .fg(p.bar_bg)
                .add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    let paragraph = Paragraph::new(visual.lines.clone())
        .scroll(((visual.scroll.min(u16::MAX as usize)) as u16, 0));
    frame.render_widget(paragraph, chunks[1]);

    let footer = " j/k scroll   Space/b page   g/G top/bottom   q close   V recapture ";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::default().bg(p.bar_bg).fg(p.dim),
        ))),
        chunks[2],
    );
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let mut spans: Vec<Span> = Vec::new();
    for (index, _) in app.tabs.iter().enumerate() {
        let title = util::truncate(&app.tab_title(index), 26);
        let label = format!(" {} {} ", index + 1, title);
        if index == app.active {
            spans.push(Span::styled(
                label,
                Style::default()
                    .bg(p.tab_active_bg)
                    .fg(p.tab_active_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                label,
                Style::default().fg(p.dim).bg(Color::Reset),
            ));
        }
        spans.push(Span::styled("│", Style::default().fg(p.border)));
    }
    spans.push(Span::styled(" + ", Style::default().fg(p.dim)));
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_address(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    if let Some(editor) = app.prompt_editor() {
        let (label, prefix) = match &app.mode {
            Mode::Address { new_tab, .. } => {
                ("OPEN", if *new_tab { "open (new tab)" } else { "open" })
            }
            Mode::Command { .. } => ("COMMAND", "command"),
            Mode::Find { .. } => ("FIND", "find"),
            Mode::FormEdit { .. } => ("EDIT", "edit"),
            Mode::Normal => ("", ""),
        };
        let _ = prefix;
        let label_text = format!(" {label} ");
        let label_width = util::width(&label_text);
        let max_text = (area.width as usize).saturating_sub(label_width + 2);
        let (visible, cursor_col) = editor.visible(max_text);
        let line = Line::from(vec![
            Span::styled(
                label_text,
                Style::default()
                    .bg(p.accent)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {visible}"), Style::default().fg(p.text)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        let x = area.x + label_width as u16 + 1 + cursor_col as u16;
        let x = x.min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(x, area.y));
        return;
    }

    let page = if app.tabs.is_empty() {
        None
    } else {
        Some(app.current_page())
    };
    let mut spans = vec![Span::styled(
        " NORMAL ",
        Style::default()
            .bg(p.accent)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    )];
    if app.is_loading() {
        spans.push(Span::styled(
            format!(" {} ", app.spinner()),
            Style::default().fg(p.accent),
        ));
    } else {
        spans.push(Span::raw(" "));
    }
    if let Some(page) = page {
        let mut url = page.url.to_string();
        if page.reader {
            url.push_str("  [reader]");
        }
        if page.show_source {
            url.push_str("  [source]");
        }
        if page.readable {
            url.push_str("  [reader]");
        }
        let available = area.width.saturating_sub(12) as usize;
        spans.push(Span::styled(
            util::truncate(&url, available.max(10)),
            Style::default().fg(p.text),
        ));
        if app.store.is_bookmarked(page.url.as_str()) {
            spans.push(Span::styled(
                "  [bookmarked]",
                Style::default().fg(p.success),
            ));
        }
        if page.status >= 400 {
            spans.push(Span::styled(
                format!("  HTTP {}", page.status),
                Style::default().fg(p.danger).add_modifier(Modifier::BOLD),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_content(frame: &mut Frame, area: Rect, app: &mut App, p: &Palette) {
    let inner = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    app.content_area = inner;
    if app.tabs.is_empty() {
        return;
    }

    let total = app.current_doc().lines.len();
    let height = inner.height as usize;
    let max_scroll = total.saturating_sub(height);
    {
        let tab = app.current_tab_mut();
        tab.scroll = tab.scroll.min(max_scroll);
    }
    let scroll = app.current_tab().scroll;
    let selected = app.current_tab().link_sel;
    let doc = app.current_doc();

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for line in doc.lines.iter().skip(scroll).take(height) {
        lines.push(render_line(line, selected, p));
    }
    let paragraph = Paragraph::new(lines).block(Block::default().padding(Padding::horizontal(0)));
    frame.render_widget(paragraph, inner);
}

fn render_line(line: &RLine, selected: Option<usize>, p: &Palette) -> Line<'static> {
    if line.spans.is_empty() {
        return Line::from("");
    }
    let mut spans: Vec<Span> = Vec::with_capacity(line.spans.len());
    for span in &line.spans {
        let mut style = span_style(span.style, p);
        if span.link.is_some() && span.link == selected {
            style = style.bg(p.sel_bg).fg(p.sel_fg).add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(span.text.clone(), style));
    }
    Line::from(spans)
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(30)])
        .split(area);

    let (message, style) = if app.is_loading() {
        (
            format!(" {} {}", app.spinner(), app.status),
            Style::default().fg(p.accent),
        )
    } else {
        let color = match app.status_tone {
            StatusTone::Info => p.dim,
            StatusTone::Success => p.success,
            StatusTone::Error => p.danger,
        };
        (format!(" {}", app.status), Style::default().fg(color))
    };
    let width = chunks[0].width as usize;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            util::truncate(&message, width),
            style,
        )))
        .style(Style::default().bg(p.bar_bg)),
        chunks[0],
    );

    let right = if app.tabs.is_empty() {
        String::new()
    } else {
        let doc = app.current_doc();
        let height = app.content_area.height.max(1) as usize;
        let max_scroll = doc.lines.len().saturating_sub(height);
        let pct = if max_scroll == 0 {
            100
        } else {
            (app.current_tab().scroll * 100)
                .checked_div(max_scroll)
                .unwrap_or(100)
                .min(100)
        };
        if let Some(sel) = app.current_tab().link_sel {
            format!("{}%  link {}/{} ", pct, sel + 1, doc.links.len())
        } else {
            format!("{}%  {} links ", pct, doc.links.len())
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(p.bar_fg),
        )))
        .alignment(Alignment::Right)
        .style(Style::default().bg(p.bar_bg)),
        chunks[1],
    );
}

fn draw_hints(frame: &mut Frame, area: Rect, app: &App, p: &Palette) {
    let hints = if app.overlay.is_some() {
        " j/k move   Enter select   d delete   Esc close "
    } else if app.mode.is_prompt() {
        " Enter confirm   Esc cancel   Ctrl+U clear   Ctrl+W delete word "
    } else {
        " q quit  o open  t tab  Tab link  Enter follow  H/L back/fwd  / find  f forms  ? help "
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            util::truncate(hints, area.width as usize),
            Style::default().fg(p.dim),
        ))),
        area,
    );
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn draw_overlay(frame: &mut Frame, area: Rect, app: &mut App, p: &Palette) {
    let width = area.width.saturating_sub(6).clamp(24, 96);
    let height = area.height.saturating_sub(4).clamp(6, 26);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);

    let Some(overlay) = app.overlay.as_ref() else {
        return;
    };
    let title = match overlay {
        Overlay::Help { .. } => " Help ",
        Overlay::Bookmarks { .. } => " Bookmarks ",
        Overlay::History { .. } => " History ",
        Overlay::Links { .. } => " Links ",
        Overlay::Forms { .. } => " Forms ",
        Overlay::Downloads { .. } => " Downloads ",
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.border))
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    match overlay {
        Overlay::Help { scroll } => {
            let lines = help_lines(p);
            let max = lines.len().saturating_sub(1);
            let paragraph = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll(((*scroll).min(max) as u16, 0));
            frame.render_widget(paragraph, inner);
        }
        Overlay::Bookmarks { sel } => {
            let rows = app
                .store
                .bookmarks
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let line = Line::from(vec![
                        Span::styled(format!("{:3}  ", i + 1), Style::default().fg(p.dim)),
                        Span::styled(b.title.clone(), Style::default().fg(p.text)),
                        Span::raw("  "),
                        Span::styled(b.url.clone(), Style::default().fg(p.dim)),
                    ]);
                    (line, Some(i))
                })
                .collect::<Vec<_>>();
            draw_rows(
                frame,
                inner,
                rows,
                *sel,
                p,
                "No bookmarks yet — press m on a page.",
            );
        }
        Overlay::History { sel } => {
            let rows = app
                .store
                .history
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let line = Line::from(vec![
                        Span::styled(format!("{:3}  ", i + 1), Style::default().fg(p.dim)),
                        Span::styled(h.title.clone(), Style::default().fg(p.text)),
                        Span::raw("  "),
                        Span::styled(h.url.clone(), Style::default().fg(p.dim)),
                        Span::styled(
                            format!("  {}", util::format_ts(h.visited)),
                            Style::default().fg(p.border),
                        ),
                    ]);
                    (line, Some(i))
                })
                .collect::<Vec<_>>();
            draw_rows(frame, inner, rows, *sel, p, "No history yet.");
        }
        Overlay::Links { sel } => {
            let doc = app.current_doc();
            let rows = doc
                .links
                .iter()
                .enumerate()
                .map(|(i, link)| {
                    let label = if link.text.is_empty() {
                        link.url.clone()
                    } else {
                        format!("{}  {}", link.text, link.url)
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{:3}  ", i + 1), Style::default().fg(p.accent)),
                        Span::styled(label, Style::default().fg(p.link)),
                    ]);
                    (line, Some(i))
                })
                .collect::<Vec<_>>();
            draw_rows(frame, inner, rows, *sel, p, "This page has no links.");
        }
        Overlay::Forms { sel } => {
            let doc = app.current_doc();
            let mut rows: Vec<(Line<'static>, Option<usize>)> = Vec::new();
            let mut selectable = 0usize;
            for (form_idx, form) in doc.forms.iter().enumerate() {
                rows.push((
                    Line::from(vec![
                        Span::styled(
                            format!("Form {}  ", form_idx + 1),
                            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{} {}", form.method, form.action),
                            Style::default().fg(p.dim),
                        ),
                    ]),
                    None,
                ));
                for (field_idx, field) in form.fields.iter().enumerate() {
                    let marker = match field.kind {
                        crate::render::FieldKind::Checkbox => {
                            if field.checked {
                                "[x] "
                            } else {
                                "[ ] "
                            }
                        }
                        crate::render::FieldKind::Radio => {
                            if field.checked {
                                "(x) "
                            } else {
                                "( ) "
                            }
                        }
                        crate::render::FieldKind::Submit => "▶ ",
                        crate::render::FieldKind::Hidden => "· ",
                        _ => "  ",
                    };
                    let mut spans = vec![
                        Span::styled(format!("  {marker}"), Style::default().fg(p.accent)),
                        Span::styled(field.label.clone(), Style::default().fg(p.text)),
                    ];
                    match field.kind {
                        crate::render::FieldKind::Text
                        | crate::render::FieldKind::Password
                        | crate::render::FieldKind::TextArea
                        | crate::render::FieldKind::Select => {
                            spans.push(Span::styled(
                                format!(": {}", field.display_value()),
                                Style::default().fg(p.code),
                            ));
                        }
                        crate::render::FieldKind::Hidden => {
                            spans.push(Span::styled(
                                " (hidden)".to_string(),
                                Style::default().fg(p.dim),
                            ));
                        }
                        _ => {}
                    }
                    let _ = field_idx;
                    rows.push((Line::from(spans), Some(selectable)));
                    selectable += 1;
                }
            }
            draw_rows(
                frame,
                inner,
                rows,
                *sel,
                p,
                "This page has no forms. Enter edits a field, Space toggles, s submits.",
            );
        }
        Overlay::Downloads { sel } => {
            let rows = app
                .downloads
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let progress = if d.done {
                        if d.error.is_some() {
                            "failed".to_string()
                        } else {
                            format!("done · {}", util::human_bytes(d.received))
                        }
                    } else {
                        match d.total {
                            Some(total) if total > 0 => {
                                let pct = (d.received * 100 / total).min(100);
                                format!(
                                    "{}%  {} / {}",
                                    pct,
                                    util::human_bytes(d.received),
                                    util::human_bytes(total)
                                )
                            }
                            _ => format!("{} received", util::human_bytes(d.received)),
                        }
                    };
                    let line = Line::from(vec![
                        Span::styled(format!("{:3}  ", i + 1), Style::default().fg(p.dim)),
                        Span::styled(d.name.clone(), Style::default().fg(p.text)),
                        Span::styled(
                            format!("  {progress}"),
                            Style::default().fg(if d.error.is_some() {
                                p.danger
                            } else if d.done {
                                p.success
                            } else {
                                p.accent
                            }),
                        ),
                        Span::styled(format!("  {}", d.url), Style::default().fg(p.border)),
                    ]);
                    (line, Some(i))
                })
                .collect::<Vec<_>>();
            draw_rows(
                frame,
                inner,
                rows,
                *sel,
                p,
                "No downloads yet — press d on a page.",
            );
        }
    }
}

fn draw_rows(
    frame: &mut Frame,
    area: Rect,
    rows: Vec<(Line<'static>, Option<usize>)>,
    selected: usize,
    p: &Palette,
    empty: &str,
) {
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                empty.to_string(),
                Style::default().fg(p.dim),
            )))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let height = area.height as usize;
    let selected_row = rows
        .iter()
        .position(|(_, idx)| *idx == Some(selected))
        .unwrap_or(0);
    let offset = selected_row.saturating_sub(height.saturating_sub(1));
    let mut lines = Vec::new();
    for (row, (line, idx)) in rows.iter().enumerate().skip(offset).take(height) {
        let _ = row;
        if *idx == Some(selected) {
            let text = line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            lines.push(Line::from(Span::styled(
                format!("▸ {text}"),
                Style::default()
                    .bg(p.sel_bg)
                    .fg(p.sel_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(line.clone());
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn help_lines(p: &Palette) -> Vec<Line<'static>> {
    let h = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))
    };
    let k = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(format!("  {key:<18}"), Style::default().fg(p.link)),
            Span::styled(desc.to_string(), Style::default().fg(p.text)),
        ])
    };
    vec![
        h("Panzki — terminal web browser"),
        Line::from(Span::styled(
            "  by panz-08 & nakki-kun · written in Rust",
            Style::default().fg(p.dim),
        )),
        Line::from(""),
        h("Navigation"),
        k("o", "open an address (o) or open in a new tab (O)"),
        k("t / x", "new tab / close tab"),
        k("[ / ]", "previous / next tab"),
        k("Alt+1…9", "switch to a tab"),
        k("H / L", "go back / forward (also Alt+←/→)"),
        k("r", "reload the current page"),
        k("e", "reader view — useful for JavaScript-heavy sites"),
        k("E", "open the page in Browsh (real Firefox rendering)"),
        k("V", "visual preview — Firefox screenshot as coloured cells"),
        k("v", "local reader view — strips navigation and sidebars"),
        k("Tab / Shift+Tab", "select the next / previous link"),
        k("Enter", "follow the selected link"),
        k("Mouse", "scroll the wheel, click a link"),
        Line::from(""),
        h("Scrolling & search"),
        k("j / k", "scroll down / up"),
        k("Space / b", "page down / page up"),
        k("g / G", "jump to the top / bottom"),
        k("/", "find text on the page"),
        k("n / N", "next / previous match"),
        k("Esc", "clear the selected link and search"),
        Line::from(""),
        h("Pages & tools"),
        k("m", "bookmark / unbookmark the current page"),
        k("B", "bookmark list"),
        k("Ctrl+H", "history"),
        k("f", "forms on the page"),
        k("d", "download the current resource"),
        k("s", "save the page source"),
        k("u", "toggle rendered page / source"),
        k("y", "copy the URL (OSC 52)"),
        k(":", "command line"),
        Line::from(""),
        h("Commands"),
        k(":open URL", "open an address"),
        k(":tabnew [URL]", "open a new tab"),
        k(":search words", "search the web"),
        k(":reader", "reload through the reader proxy"),
        k(":browsh", "open the current page in Browsh"),
        k(":visual", "capture a Firefox screenshot of the page"),
        k(":theme light|dark", "change the color theme"),
        k(":set home=…", "change the start page"),
        k(":set search=…", "change the search URL template"),
        k(":download [path]", "download the current resource"),
        k(":save [path]", "save the page source"),
        k(":q", "quit"),
        Line::from(""),
        h("Tips"),
        Line::from(Span::styled(
            "  Type words instead of a URL to search. Prefix a path with file:// to read local HTML.",
            Style::default().fg(p.dim),
        )),
        Line::from(Span::styled(
            "  Some sites need JavaScript; press e to render them through the reader proxy.",
            Style::default().fg(p.dim),
        )),
    ]
}
