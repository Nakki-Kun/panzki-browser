mod app;
mod config;
mod engine;
mod page;
mod render;
mod start;
mod ui;
mod util;
mod visual;

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use url::Url;

use crate::config::Store;
use crate::engine::Engine;
use crate::page::Page;
use crate::render::{Doc, SpanStyle, Tone};
use crate::util::normalize_input;

struct Args {
    target: Option<String>,
    dump: bool,
    source: bool,
    links: bool,
    reader: bool,
    width: usize,
    no_color: bool,
    insecure: bool,
    browsh: bool,
    ua: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            target: None,
            dump: false,
            source: false,
            links: false,
            reader: false,
            width: 100,
            no_color: false,
            insecure: false,
            browsh: false,
            ua: None,
        }
    }
}

const HELP: &str = r#"Panzki — a terminal web browser

USAGE:
    panzki [OPTIONS] [URL | SEARCH]

ARGS:
    URL|SEARCH    Open a URL, a local file, or search the web for the words

OPTIONS:
    -d, --dump        Render the page to stdout and exit
    -s, --source      Print the raw response body and exit
    -l, --links       Print the page's links and exit
        --reader      Fetch through the reader proxy (good for JS-heavy sites)
        --browsh      Open the URL in Browsh (real Firefox rendering)
        --width N     Render width for dump modes (default 100)
        --no-color    Disable ANSI colors in dump modes
        --insecure    Accept invalid TLS certificates
        --ua STRING   Override the User-Agent
    -h, --help        Print this help
    -V, --version     Print version

EXAMPLES:
    panzki                       # start page (interactive)
    panzki example.com           # open a site
    panzki rust async book       # search the web
    panzki -d news.ycombinator.com | less
"#;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("panzki: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("panzki {}", env!("CARGO_PKG_VERSION"));
                return Ok(ExitCode::SUCCESS);
            }
            "-d" | "--dump" => args.dump = true,
            "-s" | "--source" => args.source = true,
            "-l" | "--links" => args.links = true,
            "--reader" => args.reader = true,
            "--browsh" => args.browsh = true,
            "--no-color" => args.no_color = true,
            "--insecure" => args.insecure = true,
            "--width" => {
                args.width = it
                    .next()
                    .context("--width needs a value")?
                    .parse()
                    .context("--width must be a number")?;
            }
            "--ua" => args.ua = Some(it.next().context("--ua needs a value")?),
            other if other.starts_with('-') => bail!("unknown option: {other} (try --help)"),
            other => {
                if args.target.is_some() {
                    bail!("only one URL/search argument is supported");
                }
                args.target = Some(other.to_string());
            }
        }
    }

    if args.browsh {
        let store = Store::load();
        let input = args
            .target
            .clone()
            .unwrap_or_else(|| store.config.home.clone());
        let target = normalize_input(&input, &store.config.search);
        if target.is_empty() {
            bail!("nothing to open");
        }
        let status = std::process::Command::new("browsh")
            .arg(&target)
            .status()
            .context(
                "could not launch browsh — install it from https://github.com/browsh-org/browsh",
            )?;
        return Ok(if status.success() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }

    let interactive = !(args.dump || args.source || args.links);
    if interactive {
        return app::run(args.target, args.reader, args.insecure).map(|()| ExitCode::SUCCESS);
    }

    let store = Store::load();
    let input = args
        .target
        .clone()
        .unwrap_or_else(|| store.config.home.clone());
    let target = normalize_input(&input, &store.config.search);
    if target.is_empty() {
        bail!("nothing to open");
    }

    let engine = Engine::new(
        args.ua.as_deref().or(store.config.user_agent.as_deref()),
        args.insecure,
    )?;
    let page = load_page(&engine, &store, &target, args.width.max(20), args.reader)?;

    if args.source {
        print!("{}", page.raw);
        return Ok(ExitCode::SUCCESS);
    }
    if args.links {
        for (i, link) in page.doc.links.iter().enumerate() {
            if link.text.is_empty() {
                println!("{:3}. {}", i + 1, link.url);
            } else {
                println!("{:3}. {} — {}", i + 1, link.text, link.url);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    let color = !args.no_color && std::io::stdout().is_terminal();
    if color {
        print_colored(&page.doc);
    } else {
        let text = page.doc.plain_text();
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn load_page(
    engine: &Engine,
    store: &Store,
    target: &str,
    width: usize,
    reader: bool,
) -> Result<Page> {
    let url = Url::parse(target).with_context(|| format!("invalid URL: {target}"))?;

    if url.scheme() == "panzki" {
        if url.path() == "start" || url.path().is_empty() {
            let html = start::start_page(&store.bookmarks, &store.config.home);
            return Ok(Page::from_html("panzki:start", &html, &store.config, width));
        }
        bail!("unknown internal page: {url}");
    }

    let fetched = if reader {
        engine.fetch_reader(&url)?
    } else {
        engine.fetch(&url)?
    };
    Ok(Page::from_fetched(&fetched, &store.config, width))
}

fn print_colored(doc: &Doc) {
    let mut current = SpanStyle::default();
    let mut active = false;
    for line in &doc.lines {
        for span in &line.spans {
            if span.style != current {
                if active {
                    print!("\x1b[0m");
                }
                print!("{}", ansi_for(span.style));
                current = span.style;
                active = true;
            }
            print!("{}", span.text);
        }
        if active {
            print!("\x1b[0m");
            active = false;
            current = SpanStyle::default();
        }
        println!();
    }
}

fn ansi_for(style: SpanStyle) -> String {
    let mut codes: Vec<String> = Vec::new();
    if style.bold {
        codes.push("1".into());
    }
    if style.italic {
        codes.push("3".into());
    }
    if style.underline {
        codes.push("4".into());
    }
    if style.strike {
        codes.push("9".into());
    }
    let color = match style.tone {
        Tone::Text => None,
        Tone::Dim | Tone::Quote => Some("90"),
        Tone::Accent | Tone::Heading => Some("96"),
        Tone::Link => Some("94"),
        Tone::Code => Some("93"),
    };
    if let Some(c) = color {
        codes.push(c.to_string());
    }
    if codes.is_empty() {
        "\x1b[0m".to_string()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}
