use anyhow::Result;
use url::Url;

use crate::config::Config;
use crate::engine::{Fetched, is_markdown, is_textual};
use crate::render::{self, Doc};
use crate::util::human_bytes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Html,
    Markdown,
    Plain,
    Json,
    Binary,
    Error,
}

pub struct Page {
    pub url: Url,
    pub title: String,
    pub doc: Doc,
    pub raw: String,
    pub content_type: String,
    pub status: u16,
    pub kind: PageKind,
    pub reader: bool,
    pub readable: bool,
    pub show_source: bool,
    pub source: Option<Doc>,
    pub rendered_width: usize,
}

impl Page {
    pub fn from_fetched(fetched: &Fetched, config: &Config, width: usize) -> Self {
        let ct = fetched.content_type.to_ascii_lowercase();
        let (body, reader_title) = if fetched.from_reader {
            reader_body(&fetched.text)
        } else {
            (fetched.text.clone(), None)
        };
        let text = body.as_str();
        let (kind, doc) = if fetched.from_reader || is_markdown(&ct) {
            (
                PageKind::Markdown,
                render::render_markdown(text, &fetched.url, width),
            )
        } else if ct.contains("html") || looks_like_html(text) {
            (
                PageKind::Html,
                render::render_html(text, &fetched.url, width, config.link_numbers),
            )
        } else if ct.contains("json") {
            (PageKind::Json, render::render_json(text, width))
        } else if is_textual(&ct) {
            (PageKind::Plain, render::render_plain(text, width))
        } else {
            (
                PageKind::Binary,
                render::render_message(
                    "Binary content",
                    &format!(
                        "This resource is not text.\n\nType: {}\nSize: {}\nURL: {}\n\nPress d to download it, or use :open to go elsewhere.",
                        fetched.content_type,
                        human_bytes(fetched.size() as u64),
                        fetched.url
                    ),
                    width,
                ),
            )
        };

        let title = if fetched.from_reader {
            reader_title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| doc.title.clone())
        } else if doc.title.trim().is_empty() {
            url_title(&fetched.url)
        } else {
            doc.title.clone()
        };
        Self {
            url: fetched.url.clone(),
            title,
            doc,
            raw: fetched.text.clone(),
            content_type: fetched.content_type.clone(),
            status: fetched.status,
            kind,
            reader: fetched.from_reader,
            readable: false,
            show_source: false,
            source: None,
            rendered_width: width,
        }
    }

    pub fn from_html(url: &str, html: &str, config: &Config, width: usize) -> Self {
        let parsed = Url::parse(url).unwrap_or_else(|_| Url::parse("panzki:start").unwrap());
        let doc = render::render_html(html, &parsed, width, config.link_numbers);
        let title = if doc.title.trim().is_empty() {
            url_title(&parsed)
        } else {
            doc.title.clone()
        };
        Self {
            url: parsed,
            title,
            doc,
            raw: html.to_string(),
            content_type: "text/html".to_string(),
            status: 200,
            kind: PageKind::Html,
            reader: false,
            readable: false,
            show_source: false,
            source: None,
            rendered_width: width,
        }
    }

    pub fn error(url: &Url, message: &str, width: usize) -> Self {
        let body = format!(
            "{message}\n\nURL: {url}\n\nTry again with r, go back with H, or open another address with o."
        );
        let doc = render::render_message("Could not load page", &body, width);
        Self {
            url: url.clone(),
            title: format!("Error — {}", url_title(url)),
            doc,
            raw: String::new(),
            content_type: "text/plain".to_string(),
            status: 0,
            kind: PageKind::Error,
            reader: false,
            readable: false,
            show_source: false,
            source: None,
            rendered_width: width,
        }
    }

    pub fn current_doc(&self) -> &Doc {
        if self.show_source {
            self.source.as_ref().unwrap_or(&self.doc)
        } else {
            &self.doc
        }
    }

    pub fn ensure_width(&mut self, width: usize, config: &Config) -> Result<()> {
        if self.rendered_width == width {
            return Ok(());
        }
        self.doc = match self.kind {
            PageKind::Html => {
                if self.readable {
                    render::render_article(&self.raw, &self.url, width, config.link_numbers)
                } else {
                    render::render_html(&self.raw, &self.url, width, config.link_numbers)
                }
            }
            PageKind::Markdown => render::render_markdown(&self.raw, &self.url, width),
            PageKind::Json => render::render_json(&self.raw, width),
            PageKind::Plain => render::render_plain(&self.raw, width),
            PageKind::Binary | PageKind::Error => self.doc.clone(),
        };
        self.source = None;
        self.rendered_width = width;
        Ok(())
    }

    /// Toggle a local reader view that strips navigation and sidebars.
    pub fn toggle_readable(&mut self, width: usize, config: &Config) -> bool {
        if self.kind != PageKind::Html || self.reader || self.raw.is_empty() {
            return false;
        }
        self.readable = !self.readable;
        self.doc = if self.readable {
            render::render_article(&self.raw, &self.url, width, config.link_numbers)
        } else {
            render::render_html(&self.raw, &self.url, width, config.link_numbers)
        };
        self.source = None;
        self.show_source = false;
        self.rendered_width = width;
        true
    }

    pub fn toggle_source(&mut self, width: usize) {
        if self.kind == PageKind::Binary || self.kind == PageKind::Error || self.raw.is_empty() {
            return;
        }
        if self.source.is_none() {
            self.source = Some(render::render_plain(&self.raw, width));
        }
        self.show_source = !self.show_source;
    }

    pub fn short_url(&self) -> String {
        let mut s = self.url.to_string();
        if s.starts_with("https://") {
            s = s.trim_start_matches("https://").to_string();
        } else if s.starts_with("http://") {
            s = s.trim_start_matches("http://").to_string();
        }
        s
    }
}

fn reader_body(raw: &str) -> (String, Option<String>) {
    let mut title = None;
    for line in raw.lines().take(12) {
        if let Some(value) = line.strip_prefix("Title:") {
            let value = value.trim();
            if !value.is_empty() {
                title = Some(value.to_string());
            }
        }
    }
    if let Some(index) = raw.find("Markdown Content:") {
        let body = raw[index + "Markdown Content:".len()..].trim_start();
        return (body.to_string(), title);
    }
    (raw.to_string(), title)
}

fn looks_like_html(text: &str) -> bool {
    let head = text.trim_start().to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

fn url_title(url: &Url) -> String {
    if let Some(host) = url.host_str() {
        let path = url.path().trim_end_matches('/');
        if path.is_empty() || path == "/" {
            host.to_string()
        } else {
            format!("{host}{path}")
        }
    } else {
        url.to_string()
    }
}
