# Panzki

**Panzki** is a fast, interactive web browser that lives in your terminal — written from scratch in
Rust. It turns HTML into readable, styled text, keeps links and forms usable, and gives you tabs,
history, bookmarks, downloads, and a reader mode without ever leaving the keyboard.

Built by **[panz-08](https://github.com/Panz-08)** and **[nakki-kun](https://github.com/Nakki-Kun)**.

```
 1 Panzki — start │ +                                    NORMAL  panzki:start
 # Panzki

 Panzki is a terminal web browser written in Rust. Type a URL in the address bar, or a few words to
 search the web.

 ────────────────────────────────────────────────────────────────────────────────────────────────

 ## Quick links

   • Wikipedia [1]
   • Hacker News [2]
   • Lobsters [3]
   • GitHub [4]

 Panzki is ready. Press ? for help.                                          0%  9 links
 q quit  o open  t tab  Tab link  Enter follow  H/L back/fwd  / find  f forms  ? help
```

## Features

- **Real navigation** — follow links, go back/forward, reload, and keep a per-tab history.
- **Tabs** — open, close, and switch between tabs without losing your place.
- **HTML rendering** — headings, paragraphs, lists, tables, quotes, code blocks, images, and
  inline styling (bold, italic, underline, code) rendered as clean terminal text.
- **Forms** — inspect and edit every field on a page, then submit GET or POST requests.
- **Find in page** — `/` to search, `n`/`N` to jump between matches.
- **Reader mode** — `v` strips navigation and sidebars locally; `e` fetches through a reader proxy
  for JavaScript-heavy pages.
- **Bookmarks & history** — saved to disk and available from any session.
- **Downloads** — stream any resource to disk with live progress.
- **Source view** — toggle between the rendered page and its raw HTML.
- **Mouse support** — scroll with the wheel and click links.
- **Browsh hand-off** — press `E` to open the current page in [Browsh](https://github.com/browsh-org/browsh),
  which renders a real Firefox session to the terminal.
- **Themes** — dark and light palettes, switchable at runtime.
- **Headless modes** — pipe pages to `less`, extract links, or dump raw source.
- **Cookies, compression, and charset detection** — powered by `reqwest`, `encoding_rs`, and
  `scraper`.

## Install

Panzki needs a recent stable Rust toolchain (1.85+).

```sh
git clone https://github.com/Panz-08/panzki-browser.git
cd panzki-browser
cargo build --release
./target/release/panzki
```

To install it on your `PATH`:

```sh
cargo install --path .
```

## Usage

```sh
panzki                     # open the start page
panzki example.com         # open a site
panzki rust async book     # search the web
panzki -d example.com      # render to stdout and exit
panzki -s example.com      # print the raw response body
panzki -l example.com      # list links
panzki --reader site.com   # fetch through the reader proxy
panzki --browsh site.com   # full Firefox rendering via Browsh
```

### Keys

| Key | Action |
| --- | --- |
| `o` / `O` | open an address / open in a new tab |
| `t` / `x` | new tab / close tab |
| `[` / `]` | previous / next tab |
| `Alt+1…9` | switch to a tab |
| `H` / `L` | back / forward |
| `r` | reload |
| `Tab` / `Shift+Tab` | select next / previous link |
| `Enter` | follow the selected link |
| `j` / `k` | scroll down / up |
| `Space` / `b` | page down / page up |
| `g` / `G` | jump to the top / bottom |
| `/` | find in page |
| `n` / `N` | next / previous match |
| `m` | bookmark / unbookmark |
| `B` | bookmark list |
| `Ctrl+H` | history |
| `f` | forms |
| `d` | download |
| `s` | save page source |
| `u` | toggle rendered page / source |
| `v` | local reader view |
| `e` | remote reader view |
| `E` | open the page in Browsh (real Firefox rendering) |
| `y` | copy the URL (OSC 52) |
| `:` | command line |
| `?` | help |
| `q` | quit |

### Commands

`:open`, `:tabnew`, `:tabclose`, `:back`, `:forward`, `:reload`, `:home`, `:bookmark`,
`:bookmarks`, `:history`, `:links`, `:forms`, `:download`, `:downloads`, `:save`, `:source`,
`:readable`, `:reader`, `:browsh`, `:find`, `:search`, `:theme`, `:set`, `:help`, `:q`.

## Rendering: native text vs. Browsh

Panzki is a **native text browser**, in the same family as Lynx and w3m. It parses HTML and
lays the content out as styled terminal text: headings, paragraphs, lists, tables, quotes,
code, links, and forms. It does **not** execute JavaScript or run a CSS layout engine, so
JavaScript-heavy sites can look sparse. For those pages, use `e` (reader proxy) or `v` (local
reader) to extract the readable content.

If you want a pixel-faithful browser engine inside the terminal, Panzki can hand the current
URL to **Browsh**, which drives Firefox and paints the rendered page as terminal cells. Press
`E` or run `:browsh`. Browsh must be installed separately:

```sh
# https://github.com/browsh-org/browsh
browsh --version
```

Panzki keeps its own session while Browsh is open; when Browsh exits you are returned to the
same page and tabs. This gives you both modes: fast native text for documents and articles,
and full Firefox rendering for complex web apps.

## Configuration

Panzki stores its configuration, bookmarks, and history in your platform's config directory
(for example `~/.config/panzki/` on Linux). Useful settings:

```sh
:set home=https://example.com
:set search=https://www.bing.com/search?q={query}
:set theme=light
:set link_numbers=false
:set reader_fallback=true
```

The default search engine is Bing. You can point `search` at any endpoint that accepts a `{query}`
placeholder.

## How it works

| Module | Responsibility |
| --- | --- |
| `engine.rs` | HTTP client, cookies, compression, charset detection, downloads |
| `render.rs` | HTML → styled blocks → wrapped terminal lines, links, forms |
| `page.rs` | Page model, local/remote reader views, width re-rendering |
| `app.rs` | Application state, tabs, history, input handling, commands |
| `ui.rs` | `ratatui` drawing, overlays, palettes |
| `config.rs` | Persistent settings, bookmarks, history |
| `main.rs` | CLI parsing and headless dump modes |

The renderer walks the DOM with `scraper`, builds styled segments, and lays them out with
Unicode-aware width calculations. Links and form fields are collected during the same pass, so
what you see is exactly what you can interact with.

## Development

```sh
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

## License

MIT — see [LICENSE](LICENSE). Made with care by panz-08 and nakki-kun.

## Credits

Panzki is designed and maintained by [panz-08](https://github.com/Panz-08) and
[nakki-kun](https://github.com/Nakki-Kun). Issues, ideas, and pull requests are welcome from
everyone — see [AUTHORS](AUTHORS) for the people behind the project.
