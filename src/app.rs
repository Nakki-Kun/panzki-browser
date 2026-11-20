use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::Engine as _;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::terminal;
use ratatui::layout::Rect;
use ratatui::text::Line;
use url::Url;

use crate::config::{Bookmark, Store, download_dir};
use crate::engine::{Engine, Fetched};
use crate::page::{Page, PageKind};
use crate::render::Doc;
use crate::start;
use crate::ui;
use crate::util::{self, normalize_input};
use crate::visual::{self, VisualPage};

// ---------------------------------------------------------------------------
// Small text editor used by the address bar, command line and prompts
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub chars: Vec<char>,
    pub cursor: usize,
    /// When true the next typed character replaces the whole line (used when
    /// the address bar opens with the current URL pre-filled).
    pub select_all: bool,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Self {
            chars,
            cursor,
            select_all: false,
        }
    }

    pub fn new_selected(text: &str) -> Self {
        let mut editor = Self::new(text);
        editor.select_all = true;
        editor
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    pub fn insert(&mut self, c: char) {
        if self.select_all {
            self.chars.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars().filter(|c| *c != '\n' && *c != '\r') {
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.select_all {
            self.clear();
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.select_all {
            self.clear();
            return;
        }
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.select_all = false;
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.select_all = false;
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.select_all = false;
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.select_all = false;
        self.cursor = self.chars.len();
    }

    pub fn word_left(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
    }

    pub fn word_right(&mut self) {
        let len = self.chars.len();
        while self.cursor < len && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < len && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
    }

    pub fn delete_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.chars.drain(self.cursor..end);
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.select_all = false;
    }

    /// Return the visible text and cursor column for a width-limited prompt.
    pub fn visible(&self, max_width: usize) -> (String, usize) {
        let full: String = self.chars.iter().collect();
        if max_width == 0 {
            return (String::new(), 0);
        }
        let cursor_width = self
            .chars
            .iter()
            .take(self.cursor)
            .map(|c| util::char_width(*c))
            .sum::<usize>();
        if cursor_width < max_width {
            let text = util::truncate(&full, max_width);
            return (text, cursor_width);
        }
        let mut start = self.cursor;
        let mut used = 0usize;
        while start > 0 {
            let w = util::char_width(self.chars[start - 1]);
            if used + w >= max_width {
                break;
            }
            used += w;
            start -= 1;
        }
        let visible: String = self.chars[start..].iter().collect();
        (visible, used)
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct FindState {
    pub query: String,
    pub matches: Vec<usize>,
    pub at: usize,
}

pub struct Tab {
    pub pages: Vec<Page>,
    pub index: usize,
    pub scroll: usize,
    pub link_sel: Option<usize>,
    pub find: Option<FindState>,
}

impl Tab {
    pub fn new(page: Page) -> Self {
        Self {
            pages: vec![page],
            index: 0,
            scroll: 0,
            link_sel: None,
            find: None,
        }
    }

    pub fn current(&self) -> &Page {
        &self.pages[self.index]
    }

    pub fn current_mut(&mut self) -> &mut Page {
        &mut self.pages[self.index]
    }

    pub fn push(&mut self, page: Page) {
        self.pages.truncate(self.index + 1);
        self.pages.push(page);
        if self.pages.len() > 30 {
            self.pages.remove(0);
        }
        self.index = self.pages.len() - 1;
        self.scroll = 0;
        self.link_sel = None;
        self.find = None;
    }

    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.pages.len()
    }
}

pub enum Mode {
    Normal,
    Address {
        ed: Editor,
        new_tab: bool,
    },
    Command {
        ed: Editor,
    },
    Find {
        ed: Editor,
    },
    FormEdit {
        form: usize,
        field: usize,
        ed: Editor,
    },
}

impl Mode {
    pub fn is_prompt(&self) -> bool {
        !matches!(self, Mode::Normal)
    }
}

pub enum Overlay {
    Help { scroll: usize },
    Bookmarks { sel: usize },
    History { sel: usize },
    Links { sel: usize },
    Forms { sel: usize },
    Downloads { sel: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Success,
    Error,
}

pub struct DownloadItem {
    pub name: String,
    pub url: String,
    pub path: PathBuf,
    pub total: Option<u64>,
    pub received: u64,
    pub done: bool,
    pub error: Option<String>,
}

enum DownloadMsg {
    Progress(usize, u64, Option<u64>),
    Done(usize, std::result::Result<u64, String>),
}

enum FetchRequest {
    Get {
        url: Url,
        reader: bool,
    },
    Submit {
        url: Url,
        method: String,
        fields: Vec<(String, String)>,
    },
}

struct LoadResult {
    id: u64,
    url: Url,
    new_tab: bool,
    reader: bool,
    fallback: bool,
    result: std::result::Result<Fetched, String>,
}

struct LoadState {
    id: u64,
}

struct Loader {
    tx: Sender<LoadResult>,
    rx: Receiver<LoadResult>,
    next_id: u64,
    current: Option<LoadState>,
}

impl Loader {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            next_id: 1,
            current: None,
        }
    }
}

pub struct VisualState {
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
    pub width: usize,
    pub height: usize,
    pub url: String,
}

pub struct App {
    pub store: Store,
    pub engine: Engine,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub mode: Mode,
    pub overlay: Option<Overlay>,
    pub status: String,
    pub status_tone: StatusTone,
    pub should_quit: bool,
    pub width: usize,
    pub height: usize,
    pub content_area: Rect,
    pub tick: usize,
    pub downloads: Vec<DownloadItem>,
    pub visual: Option<VisualState>,
    pub reader: bool,
    /// Set when the user asks to hand the current page to Browsh. The event
    /// loop restores the terminal, runs Browsh, then re-enters the TUI.
    browsh_request: Option<String>,
    visual_request: Option<Url>,
    download_tx: Sender<DownloadMsg>,
    download_rx: Receiver<DownloadMsg>,
    loader: Loader,
}

impl App {
    pub fn new(insecure: bool, reader: bool) -> Result<Self> {
        let store = Store::load();
        let engine = Engine::new(store.config.user_agent.as_deref(), insecure)?;
        let (w, h) = terminal::size().unwrap_or((80, 24));
        let width = (w as usize).saturating_sub(2).max(20);
        let (download_tx, download_rx) = mpsc::channel();
        let mut app = Self {
            store,
            engine,
            tabs: Vec::new(),
            active: 0,
            mode: Mode::Normal,
            overlay: None,
            status: String::new(),
            status_tone: StatusTone::Info,
            should_quit: false,
            width,
            height: h as usize,
            content_area: Rect::new(1, 3, width as u16, h.saturating_sub(5)),
            tick: 0,
            downloads: Vec::new(),
            visual: None,
            reader,
            browsh_request: None,
            visual_request: None,
            download_tx,
            download_rx,
            loader: Loader::new(),
        };
        // Always keep at least one tab so drawing and key handling never see an
        // empty tab list while the first asynchronous load is in flight.
        app.open_home();
        Ok(app)
    }

    // -- pages ------------------------------------------------------------

    pub fn current_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn current_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn current_page(&self) -> &Page {
        self.current_tab().current()
    }

    pub fn current_page_mut(&mut self) -> &mut Page {
        self.current_tab_mut().current_mut()
    }

    pub fn current_doc(&self) -> &Doc {
        self.current_page().current_doc()
    }

    pub fn tab_title(&self, index: usize) -> String {
        if index >= self.tabs.len() {
            return String::new();
        }
        let page = self.tabs[index].current();
        let title = page.title.trim();
        if title.is_empty() {
            page.short_url()
        } else {
            title.to_string()
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loader.current.is_some()
    }

    pub fn open_home(&mut self) {
        let html = start::start_page(&self.store.bookmarks, &self.store.config.home);
        let page = Page::from_html("panzki:start", &html, &self.store.config, self.width);
        self.tabs.push(Tab::new(page));
        self.active = self.tabs.len() - 1;
        self.status = "Panzki is ready. Press ? for help.".to_string();
        self.status_tone = StatusTone::Info;
        self.update_title();
    }

    pub fn navigate(&mut self, input: &str, new_tab: bool) {
        let target = normalize_input(input, &self.store.config.search);
        if target.is_empty() {
            return;
        }
        let url = match Url::parse(&target) {
            Ok(url) => url,
            Err(err) => {
                self.set_error(format!("Invalid URL: {err}"));
                return;
            }
        };
        if url.scheme() == "panzki" {
            self.open_internal(&url, new_tab);
            return;
        }
        if !matches!(url.scheme(), "http" | "https" | "file") {
            self.set_error(format!("Unsupported scheme: {}", url.scheme()));
            return;
        }
        self.start_load(
            FetchRequest::Get {
                url: url.clone(),
                reader: self.reader,
            },
            new_tab,
            url.to_string(),
            false,
        );
    }

    fn open_internal(&mut self, url: &Url, new_tab: bool) {
        if url.path() == "start" || url.path().is_empty() {
            let html = start::start_page(&self.store.bookmarks, &self.store.config.home);
            let page = Page::from_html("panzki:start", &html, &self.store.config, self.width);
            self.commit_page(page, new_tab);
        } else {
            let page = Page::error(url, "Unknown internal page.", self.width);
            self.commit_page(page, new_tab);
        }
    }

    fn start_load(&mut self, request: FetchRequest, new_tab: bool, label: String, fallback: bool) {
        let id = self.loader.next_id;
        self.loader.next_id += 1;
        let engine = self.engine.clone();
        let tx = self.loader.tx.clone();
        let (url, reader) = match &request {
            FetchRequest::Get { url, reader } => (url.clone(), *reader),
            FetchRequest::Submit { url, .. } => (url.clone(), false),
        };
        self.loader.current = Some(LoadState { id });
        self.status = format!("Loading {label} …");
        self.status_tone = StatusTone::Info;

        thread::spawn(move || {
            let result = match request {
                FetchRequest::Get { url, reader } => {
                    if reader {
                        engine.fetch_reader(&url)
                    } else {
                        engine.fetch(&url)
                    }
                }
                FetchRequest::Submit {
                    url,
                    method,
                    fields,
                } => engine.submit(&url, &method, &fields),
            };
            let _ = tx.send(LoadResult {
                id,
                url,
                new_tab,
                reader,
                fallback,
                result: result.map_err(|e| format!("{e:#}")),
            });
        });
    }

    fn commit_page(&mut self, page: Page, new_tab: bool) {
        let title = page.title.clone();
        let url = page.url.to_string();
        let binary = page.kind == PageKind::Binary;
        if new_tab || self.tabs.is_empty() {
            self.tabs.push(Tab::new(page));
            self.active = self.tabs.len() - 1;
        } else {
            self.current_tab_mut().push(page);
        }
        self.store.add_history(&title, &url);
        self.status = if title.is_empty() {
            format!("Loaded {url}")
        } else {
            title
        };
        self.status_tone = StatusTone::Info;
        if binary {
            self.status = "Binary content — press d to download it".to_string();
        }
        self.update_title();
    }

    fn poll_loads(&mut self) {
        loop {
            match self.loader.rx.try_recv() {
                Ok(result) => {
                    let is_current = self
                        .loader
                        .current
                        .as_ref()
                        .is_some_and(|state| state.id == result.id);
                    if !is_current {
                        continue;
                    }
                    let state = self.loader.current.take().unwrap();
                    match result.result {
                        Ok(fetched) => {
                            let page = Page::from_fetched(&fetched, &self.store.config, self.width);
                            let looks_js = page.kind == PageKind::Html
                                && page.raw.len() > 5_000
                                && page.doc.text.trim().len() < 200
                                && page.raw.to_ascii_lowercase().contains("<script");
                            let thin_page = looks_js;
                            let needs_reader = self.store.config.reader_fallback
                                && !result.reader
                                && !result.fallback
                                && looks_js;
                            let fallback_here = result.fallback;
                            self.commit_page(page, result.new_tab);
                            if fallback_here {
                                self.status =
                                    "Reader view loaded — JavaScript-heavy page rendered as text."
                                        .to_string();
                            } else if thin_page && !result.reader && !needs_reader {
                                self.status = "Page looks JavaScript-heavy — e: reader, E: Browsh"
                                    .to_string();
                            }
                            if needs_reader && !result.new_tab {
                                self.status =
                                    "Page needs JavaScript — loading reader view…".to_string();
                                self.start_load(
                                    FetchRequest::Get {
                                        url: result.url.clone(),
                                        reader: true,
                                    },
                                    false,
                                    result.url.to_string(),
                                    true,
                                );
                            }
                        }
                        Err(message) => {
                            if result.fallback {
                                self.set_error(format!("Reader view unavailable: {message}"));
                            } else {
                                let page = Page::error(&result.url, &message, self.width);
                                self.commit_page(page, result.new_tab);
                                self.set_error("Page could not be loaded. See details above.");
                            }
                        }
                    }
                    let _ = state;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
    }

    fn poll_downloads(&mut self) {
        loop {
            match self.download_rx.try_recv() {
                Ok(DownloadMsg::Progress(id, received, total)) => {
                    if let Some(item) = self.downloads.get_mut(id) {
                        item.received = received;
                        item.total = total;
                    }
                }
                Ok(DownloadMsg::Done(id, result)) => {
                    let mut success = None;
                    let mut failure = None;
                    if let Some(item) = self.downloads.get_mut(id) {
                        item.done = true;
                        match result {
                            Ok(size) => {
                                item.received = size;
                                item.total = Some(size);
                                success = Some(format!(
                                    "Downloaded {} → {}",
                                    item.name,
                                    item.path.display()
                                ));
                            }
                            Err(err) => {
                                item.error = Some(err.clone());
                                failure = Some(format!("Download failed: {err}"));
                            }
                        }
                    }
                    if let Some(message) = success {
                        self.set_success(message);
                    } else if let Some(message) = failure {
                        self.set_error(message);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
    }

    pub fn poll(&mut self) {
        self.poll_loads();
        self.poll_downloads();
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
        FRAMES[(self.tick / 2) % FRAMES.len()]
    }

    // -- navigation -------------------------------------------------------

    pub fn go_back(&mut self) {
        if !self.current_tab().can_back() {
            self.set_error("No previous page");
            return;
        }
        let width = self.width;
        let config = self.store.config.clone();
        {
            let tab = self.current_tab_mut();
            tab.index -= 1;
            tab.scroll = 0;
            tab.link_sel = None;
            tab.find = None;
            let _ = tab.current_mut().ensure_width(width, &config);
        }
        let title = self.current_page().title.clone();
        self.status = format!("← {title}");
        self.status_tone = StatusTone::Info;
        self.update_title();
    }

    pub fn go_forward(&mut self) {
        if !self.current_tab().can_forward() {
            self.set_error("No next page");
            return;
        }
        let width = self.width;
        let config = self.store.config.clone();
        {
            let tab = self.current_tab_mut();
            tab.index += 1;
            tab.scroll = 0;
            tab.link_sel = None;
            tab.find = None;
            let _ = tab.current_mut().ensure_width(width, &config);
        }
        let title = self.current_page().title.clone();
        self.status = format!("→ {title}");
        self.status_tone = StatusTone::Info;
        self.update_title();
    }

    pub fn reload(&mut self) {
        let (url, reader) = {
            let page = self.current_page();
            (page.url.clone(), page.reader || self.reader)
        };
        if url.scheme() == "panzki" {
            let html = start::start_page(&self.store.bookmarks, &self.store.config.home);
            let page = Page::from_html("panzki:start", &html, &self.store.config, self.width);
            self.commit_page(page, false);
            return;
        }
        self.start_load(
            FetchRequest::Get {
                url: url.clone(),
                reader,
            },
            false,
            url.to_string(),
            false,
        );
    }

    /// Ask the event loop to open the current page in Browsh, which renders a
    /// real Firefox session to the terminal.
    pub fn launch_browsh(&mut self) {
        let url = self.current_page().url.to_string();
        if url.starts_with("panzki:") {
            self.set_error("Internal pages cannot be opened in Browsh");
            return;
        }
        self.browsh_request = Some(url);
    }

    pub fn take_browsh_request(&mut self) -> Option<String> {
        self.browsh_request.take()
    }

    /// Ask the event loop to capture a Firefox screenshot of the current page
    /// and show it as coloured terminal cells.
    pub fn launch_visual(&mut self) {
        let url = self.current_page().url.clone();
        if url.scheme() == "panzki" {
            self.set_error("Internal pages cannot be captured visually");
            return;
        }
        self.visual_request = Some(url);
    }

    pub fn take_visual_request(&mut self) -> Option<Url> {
        self.visual_request.take()
    }

    pub fn set_visual(&mut self, page: VisualPage, url: String) {
        self.status = "Visual preview — j/k scroll, q closes".to_string();
        self.status_tone = StatusTone::Info;
        self.visual = Some(VisualState {
            lines: page.lines,
            scroll: 0,
            width: page.width,
            height: page.height,
            url,
        });
    }

    fn handle_visual_key(&mut self, key: KeyEvent) {
        let Some(visual) = self.visual.as_mut() else {
            return;
        };
        let viewport = self.content_area.height.max(1) as usize;
        let max_scroll = visual.height.saturating_sub(viewport);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('V') => self.visual = None,
            KeyCode::Char('j') | KeyCode::Down => {
                visual.scroll = (visual.scroll + 1).min(max_scroll)
            }
            KeyCode::Char('k') | KeyCode::Up => visual.scroll = visual.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                visual.scroll = (visual.scroll + viewport).min(max_scroll)
            }
            KeyCode::PageUp | KeyCode::Char('b') => {
                visual.scroll = visual.scroll.saturating_sub(viewport)
            }
            KeyCode::Char('g') | KeyCode::Home => visual.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => visual.scroll = max_scroll,
            _ => {}
        }
    }

    pub fn reload_reader(&mut self) {
        let url = self.current_page().url.clone();
        if url.scheme() == "panzki" {
            self.set_error("Reader view is not available for internal pages");
            return;
        }
        self.start_load(
            FetchRequest::Get {
                url: url.clone(),
                reader: true,
            },
            false,
            url.to_string(),
            false,
        );
    }

    pub fn follow_link(&mut self, index: usize) {
        let url = match self.current_doc().links.get(index) {
            Some(link) => link.url.clone(),
            None => return,
        };
        self.navigate(&url, false);
    }

    pub fn follow_selected(&mut self) {
        match self.current_tab().link_sel {
            Some(index) => self.follow_link(index),
            None => self.set_error("No link selected — press Tab to pick one"),
        }
    }

    pub fn next_link(&mut self, forward: bool) {
        let count = self.current_doc().links.len();
        if count == 0 {
            self.set_error("This page has no links");
            return;
        }
        let next = match self.current_tab().link_sel {
            None => {
                if forward {
                    0
                } else {
                    count - 1
                }
            }
            Some(current) => {
                if forward {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                }
            }
        };
        self.current_tab_mut().link_sel = Some(next);
        if let Some(line) = self.current_doc().line_of_link(next) {
            self.ensure_visible(line);
        }
    }

    pub fn ensure_visible(&mut self, line: usize) {
        let height = self.content_area.height.max(3) as usize;
        let tab = self.current_tab_mut();
        if line < tab.scroll {
            tab.scroll = line;
        } else if line >= tab.scroll + height {
            tab.scroll = line.saturating_sub(height / 3);
        }
    }

    pub fn scroll_by(&mut self, delta: i64) {
        let max = self.current_doc().lines.len().saturating_sub(1);
        let tab = self.current_tab_mut();
        let next = (tab.scroll as i64 + delta).clamp(0, max as i64);
        tab.scroll = next as usize;
    }

    pub fn scroll_page(&mut self, down: bool) {
        let amount = self.content_area.height.saturating_sub(2).max(3) as i64;
        self.scroll_by(if down { amount } else { -amount });
    }

    pub fn scroll_home(&mut self) {
        self.current_tab_mut().scroll = 0;
    }

    pub fn scroll_end(&mut self) {
        let max = self.current_doc().lines.len().saturating_sub(1);
        self.current_tab_mut().scroll = max;
    }

    pub fn apply_find(&mut self, query: &str) {
        let matches = self.current_doc().find(query);
        if matches.is_empty() {
            self.current_tab_mut().find = None;
            self.set_error(format!("No matches for “{query}”"));
            return;
        }
        let scroll = self.current_tab().scroll;
        let at = matches.iter().position(|line| *line >= scroll).unwrap_or(0);
        let line = matches[at];
        let total = matches.len();
        {
            let tab = self.current_tab_mut();
            tab.find = Some(FindState {
                query: query.to_string(),
                matches,
                at,
            });
            tab.scroll = line;
        }
        self.set_success(format!("Match {}/{} for “{query}”", at + 1, total));
    }

    pub fn find_next(&mut self, forward: bool) {
        let scroll = self.current_tab().scroll;
        let Some(find) = self.current_tab().find.as_ref() else {
            self.set_error("No active search — press /");
            return;
        };
        let total = find.matches.len();
        if total == 0 {
            return;
        }
        let at = if forward {
            (find.at + 1) % total
        } else {
            (find.at + total - 1) % total
        };
        let line = find.matches[at];
        let query = find.query.clone();
        let tab = self.current_tab_mut();
        if let Some(find) = tab.find.as_mut() {
            find.at = at;
        }
        tab.scroll = line;
        let _ = scroll;
        self.set_success(format!("Match {}/{} for “{query}”", at + 1, total));
    }

    pub fn toggle_source(&mut self) {
        let width = self.width;
        let tab = self.current_tab_mut();
        tab.current_mut().toggle_source(width);
        tab.scroll = 0;
        tab.link_sel = None;
        let showing = tab.current().show_source;
        if showing {
            self.set_success("Showing page source — press u to go back");
        } else {
            self.set_success("Showing rendered page");
        }
    }

    /// Toggle a local reader view (strips navigation and sidebars).
    pub fn toggle_readable(&mut self) {
        let width = self.width;
        let config = self.store.config.clone();
        let (changed, readable) = {
            let tab = self.current_tab_mut();
            let changed = tab.current_mut().toggle_readable(width, &config);
            if changed {
                tab.scroll = 0;
                tab.link_sel = None;
                tab.find = None;
            }
            (changed, tab.current().readable)
        };
        if !changed {
            self.set_error("Reader view is not available for this page");
        } else if readable {
            self.set_success("Reader view on — press v for the full page");
        } else {
            self.set_success("Full page view");
        }
    }

    // -- tabs -------------------------------------------------------------

    pub fn new_tab(&mut self) {
        let html = start::start_page(&self.store.bookmarks, &self.store.config.home);
        let page = Page::from_html("panzki:start", &html, &self.store.config, self.width);
        self.tabs.push(Tab::new(page));
        self.active = self.tabs.len() - 1;
        self.update_title();
    }

    pub fn close_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.set_error("Cannot close the last tab — press q to quit");
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        self.update_title();
    }

    pub fn switch_tab(&mut self, forward: bool) {
        if self.tabs.len() <= 1 {
            return;
        }
        if forward {
            self.active = (self.active + 1) % self.tabs.len();
        } else {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
        self.update_title();
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.update_title();
        }
    }

    // -- bookmarks / downloads / saving ----------------------------------

    pub fn toggle_bookmark(&mut self) {
        let (title, url) = {
            let page = self.current_page();
            (page.title.clone(), page.url.to_string())
        };
        if url.starts_with("panzki:") {
            self.set_error("Internal pages cannot be bookmarked");
            return;
        }
        if self.store.toggle_bookmark(&title, &url) {
            self.set_success(format!("Bookmarked “{title}”"));
        } else {
            self.set_success("Bookmark removed");
        }
    }

    pub fn download_current(&mut self) {
        self.download_current_to(None);
    }

    fn download_current_to(&mut self, target: Option<PathBuf>) {
        let url = self.current_page().url.clone();
        if url.scheme() == "panzki" {
            self.set_error("Internal pages cannot be downloaded");
            return;
        }
        let path = match target {
            Some(path) => expand_path(path),
            None => {
                let name = suggested_filename(&url, &self.current_page().content_type);
                unique_path(download_dir().join(name))
            }
        };
        self.start_download(url, path);
    }

    fn start_download(&mut self, url: Url, path: PathBuf) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());
        let id = self.downloads.len();
        self.downloads.push(DownloadItem {
            name: name.clone(),
            url: url.to_string(),
            path: path.clone(),
            total: None,
            received: 0,
            done: false,
            error: None,
        });
        let tx = self.download_tx.clone();
        let engine = self.engine.clone();
        self.set_success(format!("Downloading {name} …"));
        thread::spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let result = engine.download(&url, &path, |received, total| {
                if last.elapsed() >= Duration::from_millis(120) {
                    let _ = tx.send(DownloadMsg::Progress(id, received, total));
                    last = Instant::now();
                }
            });
            let _ = tx.send(DownloadMsg::Done(id, result.map_err(|e| format!("{e:#}"))));
        });
    }

    pub fn save_current(&mut self) {
        let page = self.current_page();
        let url = page.url.clone();
        if url.scheme() == "panzki" {
            self.set_error("Internal pages cannot be saved");
            return;
        }
        let ext = match page.kind {
            PageKind::Html => "html",
            PageKind::Markdown => "md",
            PageKind::Json => "json",
            _ => "txt",
        };
        let name = format!("{}.{}", util::slugify(&page.title), ext);
        let path = unique_path(download_dir().join(name));
        let raw = page.raw.clone();
        if raw.is_empty() {
            self.set_error("Nothing to save for this page");
            return;
        }
        match fs::write(&path, raw) {
            Ok(()) => self.set_success(format!("Saved → {}", path.display())),
            Err(err) => self.set_error(format!("Could not save: {err}")),
        }
    }

    // -- clipboard / title -------------------------------------------------

    pub fn copy_url(&mut self) {
        let url = self.current_page().url.to_string();
        let payload = base64::engine::general_purpose::STANDARD.encode(url.as_bytes());
        print!("\x1b]52;c;{payload}\x07");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        self.set_success("URL copied to clipboard (OSC 52)");
    }

    fn update_title(&self) {
        if self.tabs.is_empty() {
            return;
        }
        let title = self.tab_title(self.active);
        print!("\x1b]0;{} — panzki\x07", title);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_tone = StatusTone::Error;
    }

    pub fn set_success(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_tone = StatusTone::Success;
    }

    // -- commands ---------------------------------------------------------

    pub fn execute_command(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        let mut split = command.splitn(2, char::is_whitespace);
        let name = split.next().unwrap_or("").to_ascii_lowercase();
        let arg = split.next().unwrap_or("").trim().to_string();

        match name.as_str() {
            "open" | "o" => {
                if arg.is_empty() {
                    self.open_address(false);
                } else {
                    self.navigate(&arg, false);
                }
            }
            "tabnew" | "tab" | "t" => {
                if arg.is_empty() {
                    self.new_tab();
                } else {
                    self.navigate(&arg, true);
                }
            }
            "tabclose" | "x" | "close" => self.close_tab(),
            "back" | "b" => self.go_back(),
            "forward" | "fwd" => self.go_forward(),
            "reload" | "r" => self.reload(),
            "home" => {
                let home = self.store.config.home.clone();
                self.navigate(&home, false);
            }
            "bookmark" | "bm" => self.toggle_bookmark(),
            "bookmarks" | "bms" => self.open_overlay(Overlay::Bookmarks { sel: 0 }),
            "history" | "hist" => self.open_overlay(Overlay::History { sel: 0 }),
            "links" => self.open_overlay(Overlay::Links { sel: 0 }),
            "forms" => self.open_forms_overlay(),
            "downloads" | "dl" => self.open_overlay(Overlay::Downloads { sel: 0 }),
            "download" => {
                if arg.is_empty() {
                    self.download_current();
                } else {
                    let path = expand_path(PathBuf::from(arg));
                    self.download_current_to(Some(path));
                }
            }
            "save" => {
                if arg.is_empty() {
                    self.save_current();
                } else {
                    let page = self.current_page();
                    let raw = page.raw.clone();
                    let path = expand_path(PathBuf::from(arg));
                    match fs::write(&path, raw) {
                        Ok(()) => self.set_success(format!("Saved → {}", path.display())),
                        Err(err) => self.set_error(format!("Could not save: {err}")),
                    }
                }
            }
            "source" => self.toggle_source(),
            "readable" | "article" => self.toggle_readable(),
            "reader" => self.reload_reader(),
            "browsh" | "firefox" => self.launch_browsh(),
            "visual" | "screenshot" => self.launch_visual(),
            "find" => {
                if arg.is_empty() {
                    self.mode = Mode::Find {
                        ed: Editor::default(),
                    };
                } else {
                    self.apply_find(&arg);
                }
            }
            "search" => {
                if arg.is_empty() {
                    self.set_error("Usage: :search words");
                } else {
                    let url = normalize_input(&arg, &self.store.config.search);
                    self.navigate(&url, false);
                }
            }
            "theme" => {
                if arg.eq_ignore_ascii_case("light") || arg.eq_ignore_ascii_case("dark") {
                    self.store.config.theme = arg.to_ascii_lowercase();
                    let _ = self.store.save_config();
                    self.set_success(format!("Theme: {}", self.store.config.theme));
                } else {
                    self.set_error("Usage: :theme light|dark");
                }
            }
            "set" => self.set_option(&arg),
            "help" | "h" | "?" => self.open_overlay(Overlay::Help { scroll: 0 }),
            "clear" => {
                self.status.clear();
                self.status_tone = StatusTone::Info;
            }
            "q" | "quit" | "exit" => self.should_quit = true,
            other => self.set_error(format!("Unknown command: {other} — try :help")),
        }
    }

    fn set_option(&mut self, arg: &str) {
        let Some((key, value)) = arg.split_once('=') else {
            self.set_error(
                "Usage: :set key=value (home, search, ua, theme, link_numbers, reader_fallback)",
            );
            return;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match key.as_str() {
            "home" => self.store.config.home = value,
            "search" => self.store.config.search = value,
            "ua" | "user_agent" => {
                self.store.config.user_agent = if value.is_empty() { None } else { Some(value) }
            }
            "theme" => self.store.config.theme = value,
            "link_numbers" => {
                self.store.config.link_numbers = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }
            "reader_fallback" => {
                self.store.config.reader_fallback = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            }
            _ => {
                self.set_error(format!("Unknown option: {key}"));
                return;
            }
        }
        let _ = self.store.save_config();
        self.set_success(format!("{key} updated"));
    }

    fn open_address(&mut self, new_tab: bool) {
        let text = if self.tabs.is_empty() {
            self.store.config.home.clone()
        } else {
            self.current_page().url.to_string()
        };
        self.overlay = None;
        self.mode = Mode::Address {
            ed: Editor::new_selected(&text),
            new_tab,
        };
    }

    fn open_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
        self.mode = Mode::Normal;
    }

    fn open_forms_overlay(&mut self) {
        if self.current_doc().forms.is_empty() {
            self.set_error("This page has no forms");
            return;
        }
        self.open_overlay(Overlay::Forms { sel: 0 });
    }

    // -- forms ------------------------------------------------------------

    pub fn form_rows(&self) -> Vec<(usize, usize)> {
        let mut rows = Vec::new();
        for (form_idx, form) in self.current_doc().forms.iter().enumerate() {
            for field_idx in 0..form.fields.len() {
                rows.push((form_idx, field_idx));
            }
        }
        rows
    }

    pub fn selected_form_field(&self) -> Option<(usize, usize)> {
        let Overlay::Forms { sel } = self.overlay.as_ref()? else {
            return None;
        };
        self.form_rows().get(*sel).copied()
    }

    pub fn activate_form_field(&mut self) {
        let Some((form, field)) = self.selected_form_field() else {
            return;
        };
        let info = {
            let f = &self.current_doc().forms[form].fields[field];
            (f.kind, f.value.clone(), f.options.clone())
        };
        match info.0 {
            crate::render::FieldKind::Text
            | crate::render::FieldKind::Password
            | crate::render::FieldKind::TextArea => {
                self.mode = Mode::FormEdit {
                    form,
                    field,
                    ed: Editor::new(&info.1),
                };
            }
            crate::render::FieldKind::Checkbox => {
                let f = &mut self.current_doc_mut().forms[form].fields[field];
                f.checked = !f.checked;
            }
            crate::render::FieldKind::Radio => {
                let name = self.current_doc().forms[form].fields[field].name.clone();
                for f in &mut self.current_doc_mut().forms[form].fields {
                    if f.name == name {
                        f.checked = false;
                    }
                }
                let f = &mut self.current_doc_mut().forms[form].fields[field];
                f.checked = true;
            }
            crate::render::FieldKind::Select => {
                let f = &mut self.current_doc_mut().forms[form].fields[field];
                if !f.options.is_empty() {
                    let current = f
                        .options
                        .iter()
                        .position(|(v, _)| v == &f.value)
                        .unwrap_or(0);
                    f.value = f.options[(current + 1) % f.options.len()].0.clone();
                }
            }
            crate::render::FieldKind::Submit => self.submit_form(form),
            crate::render::FieldKind::Hidden => {}
        }
    }

    fn current_doc_mut(&mut self) -> &mut Doc {
        if self.current_page().show_source {
            // Source view is read-only; edits go to the rendered document.
            let page = self.current_page_mut();
            page.show_source = false;
            &mut page.doc
        } else {
            &mut self.current_page_mut().doc
        }
    }

    pub fn submit_form(&mut self, form_index: usize) {
        let Some(form) = self.current_doc().forms.get(form_index) else {
            return;
        };
        let action = form.action.clone();
        let method = form.method.clone();
        let pressed_submit = self
            .selected_form_field()
            .filter(|(f, _)| *f == form_index)
            .map(|(_, field)| field);
        let mut fields: Vec<(String, String)> = Vec::new();
        for (idx, field) in form.fields.iter().enumerate() {
            if field.name.is_empty() {
                continue;
            }
            match field.kind {
                crate::render::FieldKind::Submit => {
                    if pressed_submit == Some(idx) {
                        fields.push((field.name.clone(), field.value.clone()));
                    }
                }
                crate::render::FieldKind::Checkbox | crate::render::FieldKind::Radio => {
                    if field.checked {
                        fields.push((field.name.clone(), field.value.clone()));
                    }
                }
                crate::render::FieldKind::Hidden => {
                    fields.push((field.name.clone(), field.value.clone()));
                }
                _ => fields.push((field.name.clone(), field.value.clone())),
            }
        }
        let url = match Url::parse(&action) {
            Ok(url) => url,
            Err(err) => {
                self.set_error(format!("Invalid form action: {err}"));
                return;
            }
        };
        self.overlay = None;
        self.mode = Mode::Normal;
        self.start_load(
            FetchRequest::Submit {
                url: url.clone(),
                method: method.clone(),
                fields,
            },
            false,
            format!("{method} {url}"),
            false,
        );
    }

    // -- overlays ---------------------------------------------------------

    pub fn activate_overlay_item(&mut self) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        match overlay {
            Overlay::Help { .. } => {}
            Overlay::Bookmarks { sel } => {
                if let Some(bookmark) = self.store.bookmarks.get(sel).cloned() {
                    self.navigate(&bookmark.url, false);
                }
            }
            Overlay::History { sel } => {
                if let Some(entry) = self.store.history.get(sel).cloned() {
                    self.navigate(&entry.url, false);
                }
            }
            Overlay::Links { sel } => {
                self.follow_link(sel);
            }
            Overlay::Forms { sel } => {
                self.overlay = Some(Overlay::Forms { sel });
                self.activate_form_field();
            }
            Overlay::Downloads { sel } => {
                if let Some(item) = self.downloads.get(sel) {
                    self.set_success(format!("{} → {}", item.name, item.path.display()));
                }
            }
        }
    }

    pub fn delete_overlay_item(&mut self) {
        let Some(Overlay::Bookmarks { sel }) = self.overlay.as_ref() else {
            return;
        };
        let sel = *sel;
        if sel < self.store.bookmarks.len() {
            self.store.remove_bookmark(sel);
            let len = self.store.bookmarks.len();
            if let Some(Overlay::Bookmarks { sel }) = self.overlay.as_mut() {
                *sel = (*sel).min(len.saturating_sub(1));
            }
            self.set_success("Bookmark deleted");
        }
    }

    pub fn overlay_len(&self) -> usize {
        match &self.overlay {
            Some(Overlay::Bookmarks { .. }) => self.store.bookmarks.len(),
            Some(Overlay::History { .. }) => self.store.history.len(),
            Some(Overlay::Links { .. }) => self.current_doc().links.len(),
            Some(Overlay::Forms { .. }) => self.form_rows().len(),
            Some(Overlay::Downloads { .. }) => self.downloads.len(),
            _ => 0,
        }
    }

    pub fn overlay_selected(&self) -> usize {
        match &self.overlay {
            Some(Overlay::Bookmarks { sel })
            | Some(Overlay::History { sel })
            | Some(Overlay::Links { sel })
            | Some(Overlay::Forms { sel })
            | Some(Overlay::Downloads { sel }) => *sel,
            _ => 0,
        }
    }

    pub fn move_overlay(&mut self, delta: i64) {
        let len = self.overlay_len();
        if len == 0 {
            return;
        }
        let current = self.overlay_selected() as i64;
        let next = (current + delta).clamp(0, len as i64 - 1) as usize;
        match self.overlay.as_mut() {
            Some(Overlay::Bookmarks { sel })
            | Some(Overlay::History { sel })
            | Some(Overlay::Links { sel })
            | Some(Overlay::Forms { sel })
            | Some(Overlay::Downloads { sel }) => *sel = next,
            _ => {}
        }
    }

    pub fn overlay_home(&mut self) {
        match self.overlay.as_mut() {
            Some(Overlay::Bookmarks { sel })
            | Some(Overlay::History { sel })
            | Some(Overlay::Links { sel })
            | Some(Overlay::Forms { sel })
            | Some(Overlay::Downloads { sel }) => *sel = 0,
            _ => {}
        }
    }

    pub fn overlay_end(&mut self) {
        let len = self.overlay_len();
        if len == 0 {
            return;
        }
        match self.overlay.as_mut() {
            Some(Overlay::Bookmarks { sel })
            | Some(Overlay::History { sel })
            | Some(Overlay::Links { sel })
            | Some(Overlay::Forms { sel })
            | Some(Overlay::Downloads { sel }) => *sel = len - 1,
            _ => {}
        }
    }

    // -- input ------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.visual.is_some() {
            self.handle_visual_key(key);
            return;
        }
        if matches!(self.mode, Mode::Normal) && self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        match &mut self.mode {
            Mode::Normal => self.handle_normal_key(key),
            Mode::Address { .. } | Mode::Command { .. } | Mode::Find { .. } => {
                self.handle_prompt_key(key)
            }
            Mode::FormEdit { .. } => self.handle_form_edit_key(key),
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(Overlay::Help { scroll }) = self.overlay.as_mut() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                    self.overlay = None;
                    return;
                }
                KeyCode::Char('j') | KeyCode::Down => *scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                KeyCode::Char('G') | KeyCode::End => *scroll = 10_000,
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
            KeyCode::Char('j') | KeyCode::Down => self.move_overlay(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_overlay(-1),
            KeyCode::Char('g') | KeyCode::Home => self.overlay_home(),
            KeyCode::Char('G') | KeyCode::End => self.overlay_end(),
            KeyCode::PageDown | KeyCode::Char(' ') => self.move_overlay(10),
            KeyCode::PageUp => self.move_overlay(-10),
            KeyCode::Enter => self.activate_overlay_item(),
            KeyCode::Char('d') if !ctrl => self.delete_overlay_item(),
            KeyCode::Char('s') => {
                if let Some((form, _)) = self.selected_form_field() {
                    self.submit_form(form);
                }
            }
            KeyCode::Char('y') => {
                if let Some(Overlay::Links { sel }) = self.overlay.as_ref() {
                    if let Some(link) = self.current_doc().links.get(*sel) {
                        let url = link.url.clone();
                        let payload =
                            base64::engine::general_purpose::STANDARD.encode(url.as_bytes());
                        print!("\x1b]52;c;{payload}\x07");
                        self.set_success("Link copied to clipboard");
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && ctrl) {
            self.mode = Mode::Normal;
            return;
        }
        if key.code == KeyCode::Enter {
            self.submit_prompt();
            return;
        }
        let editor = match &mut self.mode {
            Mode::Address { ed, .. } | Mode::Command { ed } | Mode::Find { ed } => ed,
            _ => return,
        };
        match key.code {
            KeyCode::Char(c) if !ctrl && !alt => editor.insert(c),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => {
                if ctrl || alt {
                    editor.word_left()
                } else {
                    editor.left()
                }
            }
            KeyCode::Right => {
                if ctrl || alt {
                    editor.word_right()
                } else {
                    editor.right()
                }
            }
            KeyCode::Home => editor.home(),
            KeyCode::End => editor.end(),
            KeyCode::Char('a') if ctrl => editor.home(),
            KeyCode::Char('e') if ctrl => editor.end(),
            KeyCode::Char('u') if ctrl => editor.clear(),
            KeyCode::Char('w') if ctrl => editor.delete_word(),
            KeyCode::Tab => {
                if let Mode::Address { ed, .. } = &mut self.mode {
                    let prefix = ed.text();
                    if let Some(url) = self
                        .store
                        .history
                        .iter()
                        .map(|h| h.url.clone())
                        .find(|u| u.starts_with(&prefix) && u != &prefix)
                    {
                        ed.set(&url);
                    }
                }
            }
            _ => {}
        }
    }

    fn submit_prompt(&mut self) {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Address { ed, new_tab } => {
                let text = ed.text();
                if text.trim().is_empty() {
                    return;
                }
                self.navigate(&text, new_tab);
            }
            Mode::Command { ed } => self.execute_command(&ed.text()),
            Mode::Find { ed } => {
                let query = ed.text();
                if !query.is_empty() {
                    self.apply_find(&query);
                }
            }
            Mode::FormEdit { form, field, ed } => {
                let value = ed.text();
                if let Some(f) = self
                    .current_doc_mut()
                    .forms
                    .get_mut(form)
                    .and_then(|f| f.fields.get_mut(field))
                {
                    f.value = value;
                }
                self.overlay = Some(Overlay::Forms {
                    sel: self.overlay_selected(),
                });
            }
            Mode::Normal => {}
        }
    }

    fn handle_form_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && ctrl) {
            self.mode = Mode::Normal;
            return;
        }
        if key.code == KeyCode::Enter {
            self.submit_prompt();
            return;
        }
        let Mode::FormEdit { ed, .. } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Char(c) if !ctrl => ed.insert(c),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Delete => ed.delete(),
            KeyCode::Left => ed.left(),
            KeyCode::Right => ed.right(),
            KeyCode::Home => ed.home(),
            KeyCode::End => ed.end(),
            KeyCode::Char('u') if ctrl => ed.clear(),
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                let tab = self.current_tab_mut();
                tab.link_sel = None;
                tab.find = None;
            }
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.open_overlay(Overlay::Help { scroll: 0 }),
            KeyCode::Char(':') => {
                self.overlay = None;
                self.mode = Mode::Command {
                    ed: Editor::default(),
                };
            }
            KeyCode::Char('o') => self.open_address(false),
            KeyCode::Char('O') => self.open_address(true),
            KeyCode::Char('l') if ctrl => self.open_address(false),
            KeyCode::Char('r') if ctrl => self.reload(),
            KeyCode::Char('h') if ctrl => self.open_overlay(Overlay::History { sel: 0 }),
            KeyCode::Char('t') => self.new_tab(),
            KeyCode::Char('x') => self.close_tab(),
            KeyCode::Char(']') => self.switch_tab(true),
            KeyCode::Char('[') => self.switch_tab(false),
            KeyCode::Char(c) if alt && c.is_ascii_digit() => {
                let index = c.to_digit(10).unwrap_or(0) as usize;
                if index == 0 {
                    self.select_tab(9);
                } else {
                    self.select_tab(index - 1);
                }
            }
            KeyCode::Char('H') | KeyCode::Left if key.code == KeyCode::Char('H') || alt => {
                self.go_back()
            }
            KeyCode::Char('L') | KeyCode::Right if key.code == KeyCode::Char('L') || alt => {
                self.go_forward()
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('R') => self.reload(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('d') if ctrl => self.scroll_page(true),
            KeyCode::Char('u') if ctrl => self.scroll_page(false),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll_page(true),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll_page(false),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_home(),
            KeyCode::Char('G') | KeyCode::End => self.scroll_end(),
            KeyCode::Tab => self.next_link(true),
            KeyCode::BackTab => self.next_link(false),
            KeyCode::Enter => self.follow_selected(),
            KeyCode::Char('m') => self.toggle_bookmark(),
            KeyCode::Char('B') => self.open_overlay(Overlay::Bookmarks { sel: 0 }),
            KeyCode::Char('d') => self.download_current(),
            KeyCode::Char('s') => self.save_current(),
            KeyCode::Char('u') => self.toggle_source(),
            KeyCode::Char('v') => self.toggle_readable(),
            KeyCode::Char('y') => self.copy_url(),
            KeyCode::Char('f') => self.open_forms_overlay(),
            KeyCode::Char('e') => self.reload_reader(),
            KeyCode::Char('E') => self.launch_browsh(),
            KeyCode::Char('V') => self.launch_visual(),
            KeyCode::Char('/') => {
                self.overlay = None;
                self.mode = Mode::Find {
                    ed: Editor::default(),
                };
            }
            KeyCode::Char('n') => self.find_next(true),
            KeyCode::Char('N') => self.find_next(false),
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.overlay.is_some() {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                let area = self.content_area;
                if mouse.column < area.x
                    || mouse.column >= area.x + area.width
                    || mouse.row < area.y
                    || mouse.row >= area.y + area.height
                {
                    return;
                }
                let line = self.current_tab().scroll + (mouse.row - area.y) as usize;
                let link = self
                    .current_doc()
                    .lines
                    .get(line)
                    .and_then(|l| l.first_link());
                if let Some(link) = link {
                    self.follow_link(link);
                }
            }
            _ => {}
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Address { ed, .. } | Mode::Command { ed } | Mode::Find { ed } => {
                ed.insert_str(text)
            }
            Mode::FormEdit { ed, .. } => ed.insert_str(text),
            Mode::Normal => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.navigate(trimmed, false);
                }
            }
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.width = (cols as usize).saturating_sub(2).max(20);
        self.height = rows as usize;
        self.content_area = Rect::new(1, 3, cols.saturating_sub(2), rows.saturating_sub(5));
        let width = self.width;
        let config = self.store.config.clone();
        if !self.tabs.is_empty() {
            let tab = self.current_tab_mut();
            let _ = tab.current_mut().ensure_width(width, &config);
            let max = tab.current().current_doc().lines.len().saturating_sub(1);
            tab.scroll = tab.scroll.min(max);
        }
    }

    pub fn prompt_editor(&self) -> Option<&Editor> {
        match &self.mode {
            Mode::Address { ed, .. }
            | Mode::Command { ed }
            | Mode::Find { ed }
            | Mode::FormEdit { ed, .. } => Some(ed),
            Mode::Normal => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expand_path(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path
}

fn suggested_filename(url: &Url, content_type: &str) -> String {
    let last = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(last)
        .decode_utf8_lossy()
        .to_string();
    let mut name = decoded
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    name = name.trim_matches('-').to_string();
    if name.is_empty() || name == "." || name == ".." {
        name = match content_type.to_ascii_lowercase() {
            ct if ct.contains("html") => "index.html".to_string(),
            ct if ct.contains("json") => "data.json".to_string(),
            ct if ct.contains("markdown") => "page.md".to_string(),
            ct if ct.starts_with("text/") => "page.txt".to_string(),
            _ => "download.bin".to_string(),
        };
    }
    if name.len() > 100 {
        name.truncate(100);
    }
    name
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..1000 {
        let candidate = parent.join(format!("{stem}-{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

pub fn run(target: Option<String>, reader: bool, insecure: bool) -> Result<()> {
    let mut app = App::new(insecure, reader)?;
    if let Some(target) = target {
        app.navigate(&target, false);
    }

    let mut terminal = ui::init()?;
    let result = event_loop(&mut terminal, &mut app);
    ui::restore()?;
    app.store.save_all();
    result
}

fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    while !app.should_quit {
        app.poll();
        if let Some(url) = app.take_visual_request() {
            ui::restore()?;
            println!("Panzki: capturing a Firefox screenshot…");
            let size = terminal::size().unwrap_or((100, 30));
            let page = visual::capture(&url, size.0, size.1.saturating_sub(2));
            match ui::init() {
                Ok(fresh) => {
                    *terminal = fresh;
                }
                Err(err) => {
                    let _ = ui::restore();
                    eprintln!("panzki: could not restore the interface: {err:#}");
                    return Ok(());
                }
            }
            match page {
                Ok(page) => app.set_visual(page, url.to_string()),
                Err(err) => app.set_error(format!("Visual preview failed: {err:#}")),
            }
        }
        if let Some(url) = app.take_browsh_request() {
            ui::restore()?;
            let outcome = std::process::Command::new("browsh").arg(&url).status();
            match ui::init() {
                Ok(fresh) => {
                    *terminal = fresh;
                }
                Err(err) => {
                    let _ = ui::restore();
                    eprintln!("panzki: could not restore the interface after Browsh: {err:#}");
                    return Ok(());
                }
            }
            match outcome {
                Ok(status) if status.success() => {
                    app.set_success("Back in Panzki — Browsh closed");
                }
                Ok(status) => {
                    app.set_error(format!("Browsh exited with status {status}"));
                }
                Err(err) => {
                    app.set_error(format!(
                        "Could not launch browsh ({err}). Install it from https://github.com/browsh-org/browsh"
                    ));
                }
            }
        }
        terminal
            .draw(|frame| ui::draw(frame, app))
            .context("drawing the interface")?;
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != event::KeyEventKind::Release {
                        app.handle_key(key);
                    }
                }
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Resize(cols, rows) => app.resize(cols, rows),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
        }
        app.tick();
    }
    Ok(())
}

// Keep Bookmark referenced for overlay typing clarity in docs/tests.
#[allow(dead_code)]
fn _bookmark_type_hint(_: &Bookmark) {}
