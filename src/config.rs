use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util;

pub const DEFAULT_HOME: &str = "panzki:start";
pub const DEFAULT_SEARCH: &str = "https://www.bing.com/search?q={}";
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

fn default_home() -> String {
    DEFAULT_HOME.to_string()
}
fn default_search() -> String {
    DEFAULT_SEARCH.to_string()
}
fn default_theme() -> String {
    "dark".to_string()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub home: String,
    pub search: String,
    pub user_agent: Option<String>,
    pub theme: String,
    pub link_numbers: bool,
    pub reader_fallback: bool,
    pub max_history: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            home: default_home(),
            search: default_search(),
            user_agent: None,
            theme: default_theme(),
            link_numbers: true,
            reader_fallback: false,
            max_history: 400,
        }
    }
}

impl Config {
    pub fn light(&self) -> bool {
        self.theme.eq_ignore_ascii_case("light")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub added: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub visited: u64,
}

pub struct Store {
    pub dir: PathBuf,
    pub config: Config,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<HistoryEntry>,
}

impl Store {
    pub fn load() -> Self {
        let dir = config_dir();
        let config: Config = read_json(&dir.join("config.json")).unwrap_or_default();
        let bookmarks: Vec<Bookmark> = read_json(&dir.join("bookmarks.json")).unwrap_or_default();
        let history: Vec<HistoryEntry> = read_json(&dir.join("history.json")).unwrap_or_default();
        Self {
            dir,
            config,
            bookmarks,
            history,
        }
    }

    pub fn save_config(&self) -> Result<()> {
        write_json(&self.dir.join("config.json"), &self.config)
    }

    pub fn save_bookmarks(&self) -> Result<()> {
        write_json(&self.dir.join("bookmarks.json"), &self.bookmarks)
    }

    pub fn save_history(&self) -> Result<()> {
        write_json(&self.dir.join("history.json"), &self.history)
    }

    pub fn save_all(&self) {
        let _ = self.save_config();
        let _ = self.save_bookmarks();
        let _ = self.save_history();
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    pub fn toggle_bookmark(&mut self, title: &str, url: &str) -> bool {
        if let Some(pos) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(pos);
            let _ = self.save_bookmarks();
            false
        } else {
            self.bookmarks.insert(
                0,
                Bookmark {
                    title: if title.is_empty() {
                        url.to_string()
                    } else {
                        title.to_string()
                    },
                    url: url.to_string(),
                    added: util::now_ts(),
                },
            );
            let _ = self.save_bookmarks();
            true
        }
    }

    pub fn remove_bookmark(&mut self, index: usize) {
        if index < self.bookmarks.len() {
            self.bookmarks.remove(index);
            let _ = self.save_bookmarks();
        }
    }

    pub fn add_history(&mut self, title: &str, url: &str) {
        if url.starts_with("panzki:") {
            return;
        }
        if self
            .history
            .first()
            .is_some_and(|h| h.url == url && h.title == title)
        {
            return;
        }
        self.history.retain(|h| h.url != url);
        self.history.insert(
            0,
            HistoryEntry {
                title: if title.is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                url: url.to_string(),
                visited: util::now_ts(),
            },
        );
        let max = self.config.max_history.max(20);
        self.history.truncate(max);
        let _ = self.save_history();
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("panzki")
}

pub fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}
