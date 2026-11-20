use std::collections::HashMap;

use ego_tree::NodeRef;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::util::{char_width, width};

// ---------------------------------------------------------------------------
// Document model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Text,
    Dim,
    Accent,
    Link,
    Code,
    Heading,
    Quote,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub tone: Tone,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl SpanStyle {
    pub fn tone(tone: Tone) -> Self {
        Self {
            tone,
            ..Default::default()
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub fn strike(mut self) -> Self {
        self.strike = true;
        self
    }

    /// Overlay `other` on top of `self`; unset fields in `other` keep `self`.
    pub fn merge(self, other: Self) -> Self {
        Self {
            tone: if other.tone == Tone::Text {
                self.tone
            } else {
                other.tone
            },
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            underline: self.underline || other.underline,
            strike: self.strike || other.strike,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RSpan {
    pub text: String,
    pub style: SpanStyle,
    pub link: Option<usize>,
}

impl RSpan {
    pub fn new(text: impl Into<String>, style: SpanStyle, link: Option<usize>) -> Self {
        Self {
            text: text.into(),
            style,
            link,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RLine {
    pub spans: Vec<RSpan>,
}

impl RLine {
    pub fn empty() -> Self {
        Self { spans: Vec::new() }
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for span in &self.spans {
            s.push_str(&span.text);
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }

    pub fn first_link(&self) -> Option<usize> {
        self.spans.iter().find_map(|s| s.link)
    }
}

#[derive(Clone, Debug)]
pub struct LinkRef {
    pub url: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Password,
    Checkbox,
    Radio,
    Select,
    Submit,
    Hidden,
    TextArea,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: String,
    pub options: Vec<(String, String)>,
    pub checked: bool,
    pub label: String,
}

impl Field {
    pub fn display_value(&self) -> String {
        match self.kind {
            FieldKind::Password => "••••••".to_string(),
            FieldKind::Select => self
                .options
                .iter()
                .find(|(v, _)| v == &self.value)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| self.value.clone()),
            _ => self.value.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Form {
    pub action: String,
    pub method: String,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub title: String,
    pub lines: Vec<RLine>,
    pub links: Vec<LinkRef>,
    pub forms: Vec<Form>,
    pub text: String,
}

impl Doc {
    pub fn find(&self, query: &str) -> Vec<usize> {
        let q = query.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.text().to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn line_of_link(&self, link: usize) -> Option<usize> {
        self.lines.iter().position(|l| {
            l.spans
                .iter()
                .any(|s| s.link == Some(link) && !s.text.trim().is_empty())
        })
    }

    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

struct Seg {
    text: String,
    style: SpanStyle,
    link: Option<usize>,
}

struct Block {
    segs: Vec<Seg>,
    indent: usize,
    prefix: String,
    prefix_style: SpanStyle,
    pre: bool,
    gap_after: bool,
}

struct ListState {
    ordered: bool,
    counter: usize,
}

struct Builder<'a> {
    base: &'a Url,
    width: usize,
    link_numbers: bool,
    blocks: Vec<Block>,
    cur: Vec<Seg>,
    prefix: String,
    prefix_style: SpanStyle,
    space: bool,
    indent: usize,
    quote_depth: usize,
    pre: bool,
    links: Vec<LinkRef>,
    link_index: HashMap<String, usize>,
    forms: Vec<Form>,
    form_stack: Vec<usize>,
    list_stack: Vec<ListState>,
    in_table: usize,
    row_cells: Vec<Vec<Seg>>,
    article_mode: bool,
}

impl<'a> Builder<'a> {
    fn new(base: &'a Url, width: usize, link_numbers: bool) -> Self {
        Self {
            base,
            width: width.max(20),
            link_numbers,
            blocks: Vec::new(),
            cur: Vec::new(),
            prefix: String::new(),
            prefix_style: SpanStyle::tone(Tone::Accent),
            space: false,
            indent: 0,
            quote_depth: 0,
            pre: false,
            links: Vec::new(),
            link_index: HashMap::new(),
            forms: Vec::new(),
            form_stack: Vec::new(),
            list_stack: Vec::new(),
            in_table: 0,
            row_cells: Vec::new(),
            article_mode: false,
        }
    }

    fn add_link(&mut self, url: &str, text: &str) -> usize {
        if let Some(idx) = self.link_index.get(url) {
            return *idx;
        }
        let idx = self.links.len();
        self.links.push(LinkRef {
            url: url.to_string(),
            text: text.trim().to_string(),
        });
        self.link_index.insert(url.to_string(), idx);
        idx
    }

    fn push_seg(&mut self, text: impl Into<String>, style: SpanStyle, link: Option<usize>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.cur.last_mut() {
            if last.style == style && last.link == link && !last.text.ends_with('\n') {
                last.text.push_str(&text);
                return;
            }
        }
        self.cur.push(Seg { text, style, link });
    }

    fn push_word(&mut self, word: &str, style: SpanStyle, link: Option<usize>) {
        let attaches = word.chars().next().is_some_and(|c| {
            matches!(
                c,
                '.' | ',' | '!' | '?' | ':' | ';' | ')' | ']' | '}' | '%' | '\'' | '”'
            )
        });
        if self.space && !self.cur.is_empty() && !attaches {
            let last_is_break = self.cur.last().is_some_and(|s| s.text.ends_with('\n'));
            if !last_is_break {
                self.push_seg(" ", SpanStyle::default(), None);
            }
        }
        self.space = false;
        self.push_seg(word, style, link);
    }

    fn push_text(&mut self, text: &str, style: SpanStyle, link: Option<usize>) {
        if self.pre {
            if !text.is_empty() {
                self.push_seg(text, style, link);
            }
            return;
        }
        let mut word = String::new();
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !word.is_empty() {
                    self.push_word(&word, style, link);
                    word.clear();
                }
                self.space = true;
            } else if ch == '\u{00a0}' {
                // non-breaking space behaves like a normal space here
                if !word.is_empty() {
                    self.push_word(&word, style, link);
                    word.clear();
                }
                self.space = true;
            } else {
                word.push(ch);
            }
        }
        if !word.is_empty() {
            self.push_word(&word, style, link);
        }
    }

    fn hard_break(&mut self) {
        if !self.cur.is_empty() {
            self.push_seg("\n", SpanStyle::default(), None);
            self.space = false;
        }
    }

    fn flush(&mut self, gap: bool) {
        if self.cur.is_empty() {
            return;
        }
        let mut segs = std::mem::take(&mut self.cur);
        self.space = false;
        while segs.first().is_some_and(|s| s.text.trim().is_empty()) {
            segs.remove(0);
        }
        while segs.last().is_some_and(|s| s.text.trim().is_empty()) {
            segs.pop();
        }
        if segs.is_empty() {
            return;
        }
        let indent = self.indent + self.list_stack.len() * 2 + self.quote_depth * 2;
        let mut prefix = std::mem::take(&mut self.prefix);
        let mut prefix_style = self.prefix_style;
        if prefix.is_empty() && self.quote_depth > 0 {
            prefix = "▎ ".to_string();
            prefix_style = SpanStyle::tone(Tone::Quote);
        }
        self.blocks.push(Block {
            segs,
            indent,
            prefix,
            prefix_style,
            pre: self.pre,
            gap_after: gap,
        });
    }

    fn add_field(&mut self, field: Field) {
        if let Some(&form) = self.form_stack.last() {
            self.forms[form].fields.push(field);
        }
    }

    fn walk_children(
        &mut self,
        el: ElementRef<'_>,
        style: SpanStyle,
        link: Option<usize>,
        depth: usize,
    ) {
        for child in el.children() {
            self.walk(child, style, link, depth + 1);
        }
    }

    fn walk(
        &mut self,
        node: NodeRef<'_, Node>,
        style: SpanStyle,
        link: Option<usize>,
        depth: usize,
    ) {
        if depth > 350 {
            return;
        }
        match node.value() {
            Node::Text(text) => {
                self.push_text(text.text.as_ref(), style, link);
            }
            Node::Element(el) => {
                let tag = el.name.local.as_ref();
                if is_skipped(tag) || is_hidden(el) {
                    return;
                }
                if self.article_mode && matches!(tag, "nav" | "aside" | "footer" | "header") {
                    return;
                }
                let s = style;
                match tag {
                    "br" => self.hard_break(),
                    "hr" => {
                        self.flush(true);
                        let n = self.width.saturating_sub(self.indent).max(4);
                        self.push_seg("─".repeat(n), SpanStyle::tone(Tone::Dim), None);
                        self.flush(true);
                    }
                    "p" | "div" | "section" | "article" | "main" | "header" | "footer"
                    | "aside" | "nav" | "address" | "fieldset" | "figure" | "details"
                    | "summary" | "body" => {
                        self.flush(true);
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.flush(true);
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = tag.as_bytes()[1] - b'0';
                        self.flush(true);
                        self.prefix = format!("{} ", "#".repeat(level as usize));
                        self.prefix_style = SpanStyle::tone(Tone::Heading).bold();
                        let hs = s
                            .merge(SpanStyle::tone(Tone::Heading))
                            .merge(SpanStyle::default().bold());
                        for child in node.children() {
                            self.walk(child, hs, link, depth + 1);
                        }
                        self.flush(true);
                        self.prefix.clear();
                    }
                    "ul" | "ol" | "menu" => {
                        self.flush(true);
                        self.list_stack.push(ListState {
                            ordered: tag == "ol",
                            counter: 0,
                        });
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.list_stack.pop();
                        self.flush(true);
                        if let Some(last) = self.blocks.last_mut() {
                            last.gap_after = true;
                        }
                    }
                    "li" => {
                        self.flush(false);
                        let (ordered, n) = if let Some(state) = self.list_stack.last_mut() {
                            if state.ordered {
                                state.counter += 1;
                                (true, state.counter)
                            } else {
                                (false, 0)
                            }
                        } else {
                            (false, 0)
                        };
                        self.prefix = if ordered {
                            format!("{n}. ")
                        } else {
                            "• ".to_string()
                        };
                        self.prefix_style = SpanStyle::tone(Tone::Accent);
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.flush(false);
                        self.prefix.clear();
                    }
                    "blockquote" | "q" => {
                        if tag == "q" {
                            self.push_seg("“", s, link);
                        }
                        self.flush(true);
                        self.quote_depth += 1;
                        for child in node.children() {
                            self.walk(
                                child,
                                s.merge(SpanStyle::tone(Tone::Quote)),
                                link,
                                depth + 1,
                            );
                        }
                        self.flush(true);
                        self.quote_depth -= 1;
                        if tag == "q" {
                            self.push_seg("”", s, link);
                        }
                    }
                    "pre" => {
                        self.flush(true);
                        let was_pre = self.pre;
                        self.pre = true;
                        self.indent += 2;
                        for child in node.children() {
                            self.walk(child, s.merge(SpanStyle::tone(Tone::Code)), link, depth + 1);
                        }
                        self.flush(true);
                        self.indent -= 2;
                        self.pre = was_pre;
                    }
                    "code" | "kbd" | "samp" | "tt" | "var" => {
                        for child in node.children() {
                            self.walk(child, s.merge(SpanStyle::tone(Tone::Code)), link, depth + 1);
                        }
                    }
                    "strong" | "b" => {
                        for child in node.children() {
                            self.walk(child, s.merge(SpanStyle::default().bold()), link, depth + 1);
                        }
                    }
                    "em" | "i" | "cite" | "dfn" => {
                        for child in node.children() {
                            self.walk(
                                child,
                                s.merge(SpanStyle::default().italic()),
                                link,
                                depth + 1,
                            );
                        }
                    }
                    "u" | "ins" => {
                        for child in node.children() {
                            self.walk(
                                child,
                                s.merge(SpanStyle::default().underline()),
                                link,
                                depth + 1,
                            );
                        }
                    }
                    "s" | "del" | "strike" => {
                        for child in node.children() {
                            self.walk(
                                child,
                                s.merge(SpanStyle::default().strike()),
                                link,
                                depth + 1,
                            );
                        }
                    }
                    "small" | "figcaption" | "caption" => {
                        for child in node.children() {
                            self.walk(child, s.merge(SpanStyle::tone(Tone::Dim)), link, depth + 1);
                        }
                    }
                    "mark" => {
                        for child in node.children() {
                            self.walk(
                                child,
                                s.merge(SpanStyle::tone(Tone::Accent).bold()),
                                link,
                                depth + 1,
                            );
                        }
                    }
                    "a" => {
                        let href = el.attr("href").map(str::trim).filter(|h| !h.is_empty());
                        let resolved = href.and_then(|h| resolve_url(self.base, h));
                        let anchor_text = ElementRef::wrap(node)
                            .map(|e| e.text().collect::<String>())
                            .unwrap_or_default();
                        if resolved.is_none() || (anchor_text.trim().is_empty() && !has_image(node))
                        {
                            for child in node.children() {
                                self.walk(child, s, link, depth + 1);
                            }
                        } else if let Some(abs) = resolved {
                            let idx = self.add_link(&abs, anchor_text.trim());
                            let before = self.cur.len();
                            let ls = s.merge(SpanStyle::tone(Tone::Link).underline());
                            for child in node.children() {
                                self.walk(child, ls, Some(idx), depth + 1);
                            }
                            if self.cur.len() == before {
                                let label = if anchor_text.trim().is_empty() {
                                    display_url(&abs)
                                } else {
                                    anchor_text.trim().to_string()
                                };
                                self.push_word(&label, ls, Some(idx));
                            }
                            if self.link_numbers {
                                if !self.cur.is_empty() {
                                    self.push_seg(" ", SpanStyle::default(), None);
                                }
                                self.push_seg(
                                    format!("[{}]", idx + 1),
                                    SpanStyle::tone(Tone::Dim),
                                    Some(idx),
                                );
                                self.space = true;
                            }
                        }
                    }
                    "img" => {
                        let alt = el.attr("alt").unwrap_or("").trim();
                        let src = el.attr("src").unwrap_or("");
                        let label = if !alt.is_empty() {
                            alt.to_string()
                        } else {
                            file_name(src)
                        };
                        let label = if label.is_empty() {
                            "image".to_string()
                        } else {
                            label
                        };
                        self.push_seg(
                            format!("[img: {label}]"),
                            SpanStyle::tone(Tone::Dim).italic(),
                            link,
                        );
                    }
                    "input" => {
                        self.handle_input(el, s, link);
                    }
                    "textarea" => {
                        let name = field_label(el);
                        let value = ElementRef::wrap(node)
                            .map(|e| e.text().collect::<String>())
                            .unwrap_or_default();
                        let field = Field {
                            name: el.attr("name").unwrap_or("").to_string(),
                            kind: FieldKind::TextArea,
                            value: value.trim().to_string(),
                            options: Vec::new(),
                            checked: false,
                            label: name.clone(),
                        };
                        self.add_field(field);
                        self.push_seg(format!("[ {name} ]"), SpanStyle::tone(Tone::Dim), link);
                    }
                    "select" => {
                        let label = field_label(el);
                        let name = el.attr("name").unwrap_or("").to_string();
                        let mut options = Vec::new();
                        let mut selected = String::new();
                        for opt in node.children().filter_map(ElementRef::wrap) {
                            if opt.value().name.local.to_string() != "option" {
                                continue;
                            }
                            let value = opt
                                .value()
                                .attr("value")
                                .map(str::to_string)
                                .unwrap_or_else(|| {
                                    opt.text().collect::<String>().trim().to_string()
                                });
                            let text = opt.text().collect::<String>().trim().to_string();
                            if selected.is_empty() || opt.value().attr("selected").is_some() {
                                selected = value.clone();
                            }
                            options.push((value, text));
                        }
                        self.add_field(Field {
                            name,
                            kind: FieldKind::Select,
                            value: selected.clone(),
                            options: options.clone(),
                            checked: false,
                            label: label.clone(),
                        });
                        let shown = options
                            .iter()
                            .find(|(v, _)| v == &selected)
                            .map(|(_, l)| l.clone())
                            .unwrap_or(selected);
                        self.push_seg(
                            format!("[ {label}: {shown} ▾ ]"),
                            SpanStyle::tone(Tone::Dim),
                            link,
                        );
                    }
                    "button" => {
                        let text = ElementRef::wrap(node)
                            .map(|e| e.text().collect::<String>())
                            .unwrap_or_default();
                        let text = text.trim();
                        let label = if text.is_empty() { "Submit" } else { text };
                        let kind = el.attr("type").unwrap_or("submit").to_ascii_lowercase();
                        if kind == "submit" || kind == "button" {
                            self.add_field(Field {
                                name: el.attr("name").unwrap_or("").to_string(),
                                kind: FieldKind::Submit,
                                value: el.attr("value").unwrap_or(label).to_string(),
                                options: Vec::new(),
                                checked: false,
                                label: label.to_string(),
                            });
                        }
                        self.push_seg(
                            format!("[ {label} ]"),
                            SpanStyle::tone(Tone::Accent).bold(),
                            link,
                        );
                    }
                    "iframe" | "frame" => {
                        let src = el.attr("src").unwrap_or("");
                        let abs = resolve_url(self.base, src);
                        let idx = abs.as_deref().map(|u| self.add_link(u, "iframe"));
                        self.push_seg(
                            "[iframe]".to_string(),
                            SpanStyle::tone(Tone::Dim).italic(),
                            idx.or(link),
                        );
                        if let Some(abs) = abs {
                            self.push_seg(format!(" {abs}"), SpanStyle::tone(Tone::Dim), idx);
                        }
                    }
                    "video" | "audio" | "source" | "track" => {
                        let src = el.attr("src").unwrap_or("");
                        if !src.is_empty() {
                            self.push_seg(
                                format!("[{}: {}]", tag, file_name(src)),
                                SpanStyle::tone(Tone::Dim),
                                link,
                            );
                        }
                    }
                    "object" | "embed" => {
                        self.push_seg(
                            format!("[{tag}]"),
                            SpanStyle::tone(Tone::Dim).italic(),
                            link,
                        );
                    }
                    "form" => {
                        let action = el
                            .attr("action")
                            .map(str::trim)
                            .filter(|a| !a.is_empty())
                            .and_then(|a| resolve_url(self.base, a))
                            .unwrap_or_else(|| self.base.to_string());
                        let method = el.attr("method").unwrap_or("get").to_ascii_uppercase();
                        let method = if method == "POST" { "POST" } else { "GET" };
                        self.forms.push(Form {
                            action,
                            method: method.to_string(),
                            fields: Vec::new(),
                        });
                        let idx = self.forms.len() - 1;
                        self.form_stack.push(idx);
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.form_stack.pop();
                    }
                    "table" => {
                        self.flush(true);
                        self.in_table += 1;
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.in_table -= 1;
                        self.flush(true);
                        if let Some(last) = self.blocks.last_mut() {
                            last.gap_after = true;
                        }
                    }
                    "tr" => {
                        self.flush(false);
                        self.row_cells.clear();
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                        self.flush(false);
                        let cells = std::mem::take(&mut self.row_cells);
                        let mut merged: Vec<Seg> = Vec::new();
                        for cell in cells {
                            if cell.iter().all(|seg| seg.text.trim().is_empty()) {
                                continue;
                            }
                            if !merged.is_empty() {
                                merged.push(Seg {
                                    text: "  │  ".to_string(),
                                    style: SpanStyle::tone(Tone::Dim),
                                    link: None,
                                });
                            }
                            merged.extend(cell);
                        }
                        if !merged.is_empty() {
                            self.cur = merged;
                            self.space = false;
                            self.flush(false);
                        }
                    }
                    "td" | "th" => {
                        self.flush(false);
                        let cs = if tag == "th" {
                            s.merge(SpanStyle::default().bold())
                        } else {
                            s
                        };
                        for child in node.children() {
                            self.walk(child, cs, link, depth + 1);
                        }
                        let cell = std::mem::take(&mut self.cur);
                        self.space = false;
                        if self.in_table > 0 {
                            self.row_cells.push(cell);
                        } else {
                            self.cur = cell;
                            self.flush(false);
                        }
                    }
                    "thead" | "tbody" | "tfoot" | "colgroup" | "col" => {
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                    }
                    "dl" | "dt" | "dd" => {
                        if tag == "dd" {
                            self.flush(true);
                            self.indent += 2;
                            for child in node.children() {
                                self.walk(child, s, link, depth + 1);
                            }
                            self.indent -= 2;
                            self.flush(true);
                        } else if tag == "dt" {
                            self.flush(true);
                            for child in node.children() {
                                self.walk(
                                    child,
                                    s.merge(SpanStyle::default().bold()),
                                    link,
                                    depth + 1,
                                );
                            }
                            self.flush(true);
                        } else {
                            for child in node.children() {
                                self.walk(child, s, link, depth + 1);
                            }
                        }
                    }
                    "legend" | "label" => {
                        let ls = if tag == "legend" {
                            s.merge(SpanStyle::default().bold())
                        } else {
                            s
                        };
                        for child in node.children() {
                            self.walk(child, ls, link, depth + 1);
                        }
                    }
                    _ => {
                        for child in node.children() {
                            self.walk(child, s, link, depth + 1);
                        }
                    }
                }
            }
            _ => {
                for child in node.children() {
                    self.walk(child, style, link, depth + 1);
                }
            }
        }
    }

    fn handle_input(&mut self, el: &scraper::node::Element, style: SpanStyle, link: Option<usize>) {
        let kind_raw = el.attr("type").unwrap_or("text").to_ascii_lowercase();
        let name = el.attr("name").unwrap_or("").to_string();
        let default_value = if matches!(kind_raw.as_str(), "checkbox" | "radio") {
            "on"
        } else {
            ""
        };
        let value = el.attr("value").unwrap_or(default_value).to_string();
        let label = field_label(el);
        let checked = el.attr("checked").is_some();
        let kind = match kind_raw.as_str() {
            "password" => FieldKind::Password,
            "checkbox" => FieldKind::Checkbox,
            "radio" => FieldKind::Radio,
            "submit" => FieldKind::Submit,
            "hidden" => FieldKind::Hidden,
            "textarea" => FieldKind::TextArea,
            _ => FieldKind::Text,
        };

        self.add_field(Field {
            name,
            kind,
            value: value.clone(),
            options: Vec::new(),
            checked,
            label: label.clone(),
        });

        match kind {
            FieldKind::Hidden => {}
            FieldKind::Checkbox => {
                self.push_seg(
                    if checked { "[x] " } else { "[ ] " },
                    SpanStyle::tone(Tone::Accent),
                    link,
                );
                self.push_seg(label, style, link);
            }
            FieldKind::Radio => {
                self.push_seg(
                    if checked { "(x) " } else { "( ) " },
                    SpanStyle::tone(Tone::Accent),
                    link,
                );
                self.push_seg(label, style, link);
            }
            FieldKind::Submit => {
                let text = if value.is_empty() { label } else { value };
                self.push_seg(
                    format!("[ {text} ]"),
                    SpanStyle::tone(Tone::Accent).bold(),
                    link,
                );
            }
            FieldKind::Password => {
                self.push_seg(
                    format!("[ {}: •••••• ]", label),
                    SpanStyle::tone(Tone::Dim),
                    link,
                );
            }
            _ => {
                if value.is_empty() {
                    self.push_seg(format!("[ {label} ]"), SpanStyle::tone(Tone::Dim), link);
                } else {
                    self.push_seg(
                        format!("[ {label}: {value} ]"),
                        SpanStyle::tone(Tone::Dim),
                        link,
                    );
                }
            }
        }
    }

    fn finish(mut self, title: String) -> Doc {
        self.flush(true);
        let width = self.width;
        let mut lines: Vec<RLine> = Vec::new();
        let mut prev_gap = false;
        for block in &self.blocks {
            if !lines.is_empty() && prev_gap && lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(RLine::empty());
            }
            layout_block(block, width, &mut lines);
            prev_gap = block.gap_after;
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let text = lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n");
        Doc {
            title: clean_title(&title),
            lines,
            links: self.links,
            forms: self.forms,
            text,
        }
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

enum Tok {
    Word {
        text: String,
        style: SpanStyle,
        link: Option<usize>,
    },
    Space,
    Break,
}

fn tokenize(segs: &[Seg]) -> Vec<Tok> {
    let mut out = Vec::new();
    for seg in segs {
        let mut word = String::new();
        for ch in seg.text.chars() {
            match ch {
                '\n' => {
                    if !word.is_empty() {
                        out.push(Tok::Word {
                            text: std::mem::take(&mut word),
                            style: seg.style,
                            link: seg.link,
                        });
                    }
                    out.push(Tok::Break);
                }
                c if c.is_whitespace() => {
                    if !word.is_empty() {
                        out.push(Tok::Word {
                            text: std::mem::take(&mut word),
                            style: seg.style,
                            link: seg.link,
                        });
                    }
                    out.push(Tok::Space);
                }
                c => word.push(c),
            }
        }
        if !word.is_empty() {
            out.push(Tok::Word {
                text: word,
                style: seg.style,
                link: seg.link,
            });
        }
    }
    out
}

fn layout_block(block: &Block, total_width: usize, out: &mut Vec<RLine>) {
    let pwidth = width(&block.prefix);
    let avail = total_width.saturating_sub(block.indent + pwidth).max(12);

    if block.pre {
        let mut raw = String::new();
        for seg in &block.segs {
            raw.push_str(&seg.text);
        }
        let raw = raw.replace('\t', "    ");
        let mut first = true;
        for line in raw.split('\n') {
            if line.trim().is_empty() && (first || out.is_empty()) {
                first = false;
                continue;
            }
            first = false;
            let mut spans = Vec::new();
            if block.indent > 0 {
                spans.push(RSpan::new(
                    " ".repeat(block.indent),
                    SpanStyle::default(),
                    None,
                ));
            }
            for (i, chunk) in split_by_width(line, avail).into_iter().enumerate() {
                let mut line_spans = spans.clone();
                if i > 0 {
                    line_spans.clear();
                    if block.indent > 0 {
                        line_spans.push(RSpan::new(
                            " ".repeat(block.indent),
                            SpanStyle::default(),
                            None,
                        ));
                    }
                }
                line_spans.push(RSpan::new(chunk, SpanStyle::tone(Tone::Code), None));
                out.push(RLine { spans: line_spans });
            }
        }
        return;
    }

    let tokens = tokenize(&block.segs);
    let mut lines: Vec<RLine> = Vec::new();
    let mut spans: Vec<RSpan> = Vec::new();
    let mut used = 0usize;
    let mut first = true;
    let mut pending_space = false;

    let begin = |spans: &mut Vec<RSpan>, first: bool| {
        if block.indent > 0 {
            spans.push(RSpan::new(
                " ".repeat(block.indent),
                SpanStyle::default(),
                None,
            ));
        }
        if pwidth > 0 {
            if first {
                spans.push(RSpan::new(block.prefix.clone(), block.prefix_style, None));
            } else {
                spans.push(RSpan::new(" ".repeat(pwidth), SpanStyle::default(), None));
            }
        }
    };

    for tok in tokens {
        match tok {
            Tok::Space => {
                if used > 0 {
                    pending_space = true;
                }
            }
            Tok::Break => {
                if !spans.is_empty() {
                    lines.push(RLine {
                        spans: std::mem::take(&mut spans),
                    });
                }
                used = 0;
                first = false;
                pending_space = false;
            }
            Tok::Word { text, style, link } => {
                let w = width(&text);
                if used > 0 && used + 1 + w > avail {
                    lines.push(RLine {
                        spans: std::mem::take(&mut spans),
                    });
                    used = 0;
                    first = false;
                    pending_space = false;
                }
                if spans.is_empty() {
                    begin(&mut spans, first);
                }
                if used > 0 && pending_space {
                    spans.push(RSpan::new(" ", SpanStyle::default(), None));
                    used += 1;
                }
                pending_space = false;
                if w > avail && used <= 1 {
                    // Hard-wrap an over-long token (URL, hash, base64 blob...).
                    let chunks = split_by_width(&text, avail);
                    let count = chunks.len();
                    for (i, chunk) in chunks.into_iter().enumerate() {
                        if i > 0 {
                            lines.push(RLine {
                                spans: std::mem::take(&mut spans),
                            });
                            first = false;
                            begin(&mut spans, first);
                        }
                        used += width(&chunk);
                        spans.push(RSpan::new(chunk, style, link));
                    }
                    let _ = count;
                } else {
                    used += w;
                    spans.push(RSpan::new(text, style, link));
                }
            }
        }
    }

    if !spans.is_empty() {
        lines.push(RLine { spans });
    }
    if lines.is_empty() && !block.prefix.is_empty() {
        let mut spans = Vec::new();
        begin(&mut spans, true);
        lines.push(RLine { spans });
    }
    out.extend(lines);
}

fn split_by_width(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return vec![text.to_string()];
    }
    if width(text) <= max {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = char_width(ch).max(1);
        if used + w > max && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(ch);
        used += w;
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub fn render_html(source: &str, base: &Url, width: usize, link_numbers: bool) -> Doc {
    let html = Html::parse_document(source);
    let title = html
        .select(&Selector::parse("title").expect("title selector"))
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default();

    let effective_base = html
        .select(&Selector::parse("base[href]").expect("base selector"))
        .next()
        .and_then(|e| e.value().attr("href"))
        .and_then(|href| resolve_url(base, href))
        .and_then(|s| Url::parse(&s).ok())
        .unwrap_or_else(|| base.clone());

    let mut builder = Builder::new(&effective_base, width, link_numbers);
    builder.walk(html.tree.root(), SpanStyle::default(), None, 0);
    builder.finish(title)
}

/// Reader-style extraction: render the largest article/main container and drop
/// navigation, sidebars, headers and footers. Falls back to the whole document
/// when no obvious content container is found.
pub fn render_article(source: &str, base: &Url, width: usize, link_numbers: bool) -> Doc {
    let html = Html::parse_document(source);
    let title = html
        .select(&Selector::parse("title").expect("title selector"))
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default();

    let effective_base = html
        .select(&Selector::parse("base[href]").expect("base selector"))
        .next()
        .and_then(|e| e.value().attr("href"))
        .and_then(|href| resolve_url(base, href))
        .and_then(|s| Url::parse(&s).ok())
        .unwrap_or_else(|| base.clone());

    let mut best: Option<ElementRef> = None;
    let mut best_len = 0usize;
    for selector in [
        "article",
        "main",
        "[role=main]",
        "#mw-content-text",
        "#main-content",
        ".post-content",
        ".entry-content",
        ".article-body",
        ".article-content",
        "#content",
    ] {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in html.select(&selector) {
            let len: usize = element.text().map(|t| t.len()).sum();
            if len > best_len {
                best_len = len;
                best = Some(element);
            }
        }
    }

    let mut builder = Builder::new(&effective_base, width, link_numbers);
    builder.article_mode = true;
    match best {
        Some(element) if best_len >= 200 => {
            builder.walk_children(element, SpanStyle::default(), None, 0);
        }
        _ => builder.walk(html.tree.root(), SpanStyle::default(), None, 0),
    }
    builder.finish(title)
}

pub fn render_plain(text: &str, width: usize) -> Doc {
    let base = Url::parse("panzki:plain").unwrap();
    let mut builder = Builder::new(&base, width, false);
    builder.pre = true;
    builder.push_seg(text, SpanStyle::tone(Tone::Code), None);
    builder.finish(String::new())
}

pub fn render_json(text: &str, width: usize) -> Doc {
    let pretty = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| text.to_string());
    let mut doc = render_plain(&pretty, width);
    doc.title = "JSON".to_string();
    doc
}

pub fn render_markdown(text: &str, base: &Url, width: usize) -> Doc {
    let mut b = Builder::new(base, width, true);
    let mut in_code = false;
    let mut code_buf = String::new();

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_code {
                b.pre = true;
                b.indent += 2;
                b.push_seg(
                    std::mem::take(&mut code_buf),
                    SpanStyle::tone(Tone::Code),
                    None,
                );
                b.indent -= 2;
                b.pre = false;
                b.flush(true);
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            b.flush(true);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("######") {
            b.flush(true);
            b.prefix = "# ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed.strip_prefix("#####") {
            b.flush(true);
            b.prefix = "# ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed.strip_prefix("####") {
            b.flush(true);
            b.prefix = "# ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed.strip_prefix("###") {
            b.flush(true);
            b.prefix = "## ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed.strip_prefix("##") {
            b.flush(true);
            b.prefix = "## ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            b.flush(true);
            b.prefix = "# ".into();
            b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
            push_markdown_inline(&mut b, rest.trim(), SpanStyle::tone(Tone::Heading).bold());
            b.flush(true);
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            b.flush(false);
            b.prefix = "• ".into();
            b.prefix_style = SpanStyle::tone(Tone::Accent);
            push_markdown_inline(&mut b, rest, SpanStyle::default());
            b.flush(false);
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            b.flush(true);
            b.quote_depth += 1;
            push_markdown_inline(&mut b, rest, SpanStyle::tone(Tone::Quote).italic());
            b.quote_depth -= 1;
            b.flush(true);
        } else if trimmed.starts_with("---") || trimmed.starts_with("***") {
            b.flush(true);
            b.push_seg(
                "─".repeat(width.saturating_sub(2).max(4)),
                SpanStyle::tone(Tone::Dim),
                None,
            );
            b.flush(true);
        } else {
            b.flush(true);
            push_markdown_inline(&mut b, trimmed, SpanStyle::default());
            b.flush(true);
        }
    }
    if in_code && !code_buf.is_empty() {
        b.pre = true;
        b.indent += 2;
        b.push_seg(code_buf, SpanStyle::tone(Tone::Code), None);
        b.indent -= 2;
        b.pre = false;
        b.flush(true);
    }
    let mut doc = b.finish(String::new());
    doc.title = "Reader view".to_string();
    doc
}

fn push_markdown_inline(b: &mut Builder<'_>, text: &str, style: SpanStyle) {
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        if let Some(end_text) = rest[start..].find("](") {
            let text_end = start + end_text;
            if let Some(end_url) = rest[text_end + 2..].find(')') {
                let url_end = text_end + 2 + end_url;
                if start > 0 {
                    b.push_text(&rest[..start], style, None);
                }
                let label = &rest[start + 1..text_end];
                let href = &rest[text_end + 2..url_end];
                if let Some(abs) = resolve_url(b.base, href) {
                    let idx = b.add_link(&abs, label);
                    b.push_word(
                        label,
                        style.merge(SpanStyle::tone(Tone::Link).underline()),
                        Some(idx),
                    );
                    if b.link_numbers {
                        b.push_seg(
                            format!("[{}]", idx + 1),
                            SpanStyle::tone(Tone::Dim),
                            Some(idx),
                        );
                    }
                } else {
                    b.push_word(label, style, None);
                }
                rest = &rest[url_end + 1..];
                continue;
            }
        }
        b.push_text(&rest[..start + 1], style, None);
        rest = &rest[start + 1..];
    }
    if !rest.is_empty() {
        b.push_text(rest, style, None);
    }
}

pub fn render_message(title: &str, body: &str, width: usize) -> Doc {
    let base = Url::parse("panzki:message").unwrap();
    let mut b = Builder::new(&base, width, false);
    b.prefix = "# ".into();
    b.prefix_style = SpanStyle::tone(Tone::Heading).bold();
    b.push_seg(title, SpanStyle::tone(Tone::Heading).bold(), None);
    b.flush(true);
    for para in body.split("\n\n") {
        b.push_text(para, SpanStyle::default(), None);
        b.flush(true);
    }
    let mut doc = b.finish(title.to_string());
    doc.title = title.to_string();
    doc
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn resolve_url(base: &Url, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with("data:")
        || lower.starts_with("blob:")
        || lower.starts_with("about:blank")
    {
        return None;
    }
    if let Ok(url) = Url::parse(href) {
        if url.scheme() == "http" || url.scheme() == "https" || url.scheme() == "file" {
            return Some(url.to_string());
        }
        if url.scheme() == "panzki" {
            return Some(url.to_string());
        }
    }
    base.join(href).ok().map(|u| u.to_string()).filter(|u| {
        u.starts_with("http://")
            || u.starts_with("https://")
            || u.starts_with("file://")
            || u.starts_with("panzki:")
    })
}

fn has_image(node: NodeRef<'_, Node>) -> bool {
    node.children().any(|child| match child.value() {
        Node::Element(el) => el.name.local.to_string() == "img" || has_image(child),
        _ => false,
    })
}

fn is_skipped(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "head"
            | "title"
            | "meta"
            | "link"
            | "svg"
            | "canvas"
            | "template"
            | "noscript"
            | "base"
            | "param"
    )
}

fn is_hidden(el: &scraper::node::Element) -> bool {
    if el.attr("hidden").is_some() {
        return true;
    }
    if let Some(style) = el.attr("style") {
        let s: String = style.chars().filter(|c| !c.is_whitespace()).collect();
        if s.contains("display:none") || s.contains("visibility:hidden") {
            return true;
        }
    }
    false
}

fn field_label(el: &scraper::node::Element) -> String {
    for key in ["aria-label", "placeholder", "name", "id", "title"] {
        if let Some(v) = el.attr(key) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    "field".to_string()
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string()
}

fn display_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string()
}

fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/dir/page.html").unwrap()
    }

    #[test]
    fn renders_basic_html() {
        let doc = render_html(
            "<html><head><title>Hi</title></head><body><h1>Hello</h1><p>World <b>bold</b>.</p></body></html>",
            &base(),
            80,
            true,
        );
        assert_eq!(doc.title, "Hi");
        let text = doc.plain_text();
        assert!(text.contains("# Hello"));
        assert!(text.contains("World bold .") || text.contains("World bold."));
    }

    #[test]
    fn collects_and_numbers_links() {
        let doc = render_html(
            r#"<p><a href="/a">One</a> and <a href="https://other.test/x">Two</a></p>"#,
            &base(),
            80,
            true,
        );
        assert_eq!(doc.links.len(), 2);
        assert_eq!(doc.links[0].url, "https://example.com/a");
        assert!(doc.plain_text().contains("[1]"));
        assert!(doc.plain_text().contains("[2]"));
    }

    #[test]
    fn builds_forms() {
        let doc = render_html(
            r#"<form action="/search" method="get"><input name="q" placeholder="Query"><input type="submit" value="Go"></form>"#,
            &base(),
            80,
            false,
        );
        assert_eq!(doc.forms.len(), 1);
        assert_eq!(doc.forms[0].action, "https://example.com/search");
        assert_eq!(doc.forms[0].fields.len(), 2);
        assert_eq!(doc.forms[0].fields[0].name, "q");
    }

    #[test]
    fn skips_scripts_and_hidden_nodes() {
        let doc = render_html(
            r#"<p>visible</p><script>alert(1)</script><div style="display:none">secret</div>"#,
            &base(),
            80,
            false,
        );
        let text = doc.plain_text();
        assert!(text.contains("visible"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn wraps_long_lines() {
        let doc = render_html(
            &format!("<p>{}</p>", "word ".repeat(50)),
            &base(),
            40,
            false,
        );
        assert!(doc.lines.len() > 3);
        for line in &doc.lines {
            assert!(width(&line.text()) <= 40);
        }
    }

    #[test]
    fn resolves_relative_urls() {
        assert_eq!(
            resolve_url(&base(), "../x?y=1").unwrap(),
            "https://example.com/x?y=1"
        );
        assert!(resolve_url(&base(), "javascript:void(0)").is_none());
    }
}
