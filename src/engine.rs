use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use encoding_rs::Encoding;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use url::Url;

use crate::config::DEFAULT_USER_AGENT;

const READER_ENDPOINT: &str = "https://r.jina.ai/";

#[derive(Clone)]
pub struct Engine {
    client: Client,
    pub user_agent: String,
}

pub struct Fetched {
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub text: String,
    pub bytes: Vec<u8>,
    pub from_reader: bool,
}

impl Fetched {
    pub fn size(&self) -> usize {
        self.bytes.len()
    }
}

impl Engine {
    pub fn new(user_agent: Option<&str>, insecure: bool) -> Result<Self> {
        let ua = user_agent.unwrap_or(DEFAULT_USER_AGENT).to_string();
        let client = Client::builder()
            .user_agent(ua.clone())
            .cookie_store(true)
            .timeout(Duration::from_secs(45))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(12))
            .danger_accept_invalid_certs(insecure)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            client,
            user_agent: ua,
        })
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
            ),
        );
        h.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9,id;q=0.8"),
        );
        h.insert("Sec-Fetch-Dest", HeaderValue::from_static("document"));
        h.insert("Sec-Fetch-Mode", HeaderValue::from_static("navigate"));
        h.insert("Sec-Fetch-Site", HeaderValue::from_static("none"));
        h.insert("Sec-Fetch-User", HeaderValue::from_static("?1"));
        h.insert("Upgrade-Insecure-Requests", HeaderValue::from_static("1"));
        h.insert("DNT", HeaderValue::from_static("1"));
        h
    }

    /// Fetch a URL. Supports http(s) and file:// URLs.
    pub fn fetch(&self, url: &Url) -> Result<Fetched> {
        match url.scheme() {
            "file" => self.read_local(url),
            "http" | "https" => {
                let resp = self
                    .client
                    .get(url.clone())
                    .headers(self.headers())
                    .send()
                    .with_context(|| format!("requesting {url}"))?;
                self.collect(resp)
            }
            other => bail!("unsupported URL scheme: {other}"),
        }
    }

    /// Submit a form (GET or POST, application/x-www-form-urlencoded).
    pub fn submit(&self, url: &Url, method: &str, fields: &[(String, String)]) -> Result<Fetched> {
        let method = method.to_ascii_uppercase();
        let encoded = encode_form(fields);
        if method == "GET" {
            let mut target = url.clone();
            {
                let mut qp = target.query_pairs_mut();
                for (k, v) in fields {
                    qp.append_pair(k, v);
                }
            }
            return self.fetch(&target);
        }

        let resp = self
            .client
            .post(url.clone())
            .headers(self.headers())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(encoded)
            .send()
            .with_context(|| format!("submitting form to {url}"))?;
        self.collect(resp)
    }

    /// Fetch a JavaScript-heavy page through the Jina reader proxy, which
    /// returns a clean Markdown version of the page.
    pub fn fetch_reader(&self, url: &Url) -> Result<Fetched> {
        let target = format!("{READER_ENDPOINT}{url}");
        let resp = self
            .client
            .get(&target)
            .header(ACCEPT, "text/plain, text/markdown;q=0.9, */*;q=0.5")
            .header(USER_AGENT, &self.user_agent)
            .send()
            .with_context(|| format!("requesting reader view for {url}"))?;
        let mut fetched = self.collect(resp)?;
        fetched.url = url.clone();
        fetched.from_reader = true;
        Ok(fetched)
    }

    fn collect(&self, resp: Response) -> Result<Fetched> {
        let final_url = resp.url().clone();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = resp.bytes().context("reading response body")?.to_vec();
        let text = decode_body(&bytes, &content_type);
        Ok(Fetched {
            url: final_url,
            status,
            content_type,
            text,
            bytes,
            from_reader: false,
        })
    }

    fn read_local(&self, url: &Url) -> Result<Fetched> {
        let path = url
            .to_file_path()
            .map_err(|_| anyhow!("invalid file URL: {url}"))?;
        let bytes =
            fs::read(&path).with_context(|| format!("reading local file {}", path.display()))?;
        let mime = mime_for_path(&path);
        let text = decode_body(&bytes, mime);
        Ok(Fetched {
            url: url.clone(),
            status: 200,
            content_type: mime.to_string(),
            text,
            bytes,
            from_reader: false,
        })
    }

    /// Stream a URL to disk, reporting progress through `progress`.
    pub fn download<F>(&self, url: &Url, path: &Path, mut progress: F) -> Result<u64>
    where
        F: FnMut(u64, Option<u64>),
    {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        if url.scheme() == "file" {
            let src = url
                .to_file_path()
                .map_err(|_| anyhow!("invalid file URL: {url}"))?;
            fs::copy(&src, path)?;
            let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            progress(size, Some(size));
            return Ok(size);
        }
        let mut resp = self
            .client
            .get(url.clone())
            .headers(self.headers())
            .send()
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .with_context(|| format!("server refused download of {url}"))?;
        let total = resp.content_length();
        let mut file =
            fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = resp.read(&mut buf).context("reading download stream")?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            written += n as u64;
            progress(written, total);
        }
        file.flush()?;
        Ok(written)
    }
}

pub fn encode_form(fields: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in fields {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode_component(k));
        out.push('=');
        out.push_str(&encode_component(v));
    }
    out
}

fn encode_component(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
        .to_string()
        .replace("%20", "+")
}

pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    if let Some(label) = charset_from_content_type(content_type)
        .or_else(|| sniff_meta_charset(bytes))
        .and_then(|l| Encoding::for_label(l.as_bytes()))
    {
        let (text, _, _) = label.decode(bytes);
        return text.into_owned();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("charset=")?;
    let rest = &content_type[idx + 8..];
    let value = rest
        .trim_start()
        .trim_start_matches('"')
        .split(|c: char| c == ';' || c == '"' || c.is_whitespace())
        .next()?
        .trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let idx = text.find("charset=")?;
    let rest = &text[idx + 8..];
    let value: String = rest
        .trim_start_matches('"')
        .trim_start_matches('\'')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if value.is_empty() { None } else { Some(value) }
}

pub fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "xhtml" => "application/xhtml+xml; charset=utf-8",
        "txt" | "log" | "conf" | "ini" | "toml" => "text/plain; charset=utf-8",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "xml" | "rss" | "atom" => "application/xml; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        "mp4" | "webm" | "mkv" => "video/mp4",
        "mp3" | "ogg" | "flac" | "wav" => "audio/mpeg",
        "zip" | "gz" | "xz" | "tar" => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

pub fn is_textual(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("text/")
        || ct.contains("html")
        || ct.contains("xml")
        || ct.contains("json")
        || ct.contains("javascript")
        || ct.contains("x-yaml")
}

pub fn is_markdown(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("markdown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding() {
        assert_eq!(
            encode_form(&[
                ("q".into(), "hello world".into()),
                ("x".into(), "a&b".into())
            ]),
            "q=hello+world&x=a%26b"
        );
    }

    #[test]
    fn decodes_meta_charset() {
        let html = b"<meta charset=\"iso-8859-1\"><p>caf\xe9</p>";
        let text = decode_body(html, "text/html");
        assert!(text.contains("café"));
    }

    #[test]
    fn parses_content_type_charset() {
        assert_eq!(
            charset_from_content_type("text/html; charset=\"UTF-8\"").as_deref(),
            Some("UTF-8")
        );
    }
}
