use crate::config::Bookmark;
use crate::util::html_escape;

/// The built-in home page. Rendered locally, never fetched from the network.
pub fn start_page(bookmarks: &[Bookmark], home_label: &str) -> String {
    let mut html = String::new();
    html.push_str(
        r#"<!doctype html><html><head><title>Panzki — start</title></head><body>
<h1>Panzki</h1>
<p><b>Panzki</b> is a terminal web browser written in Rust. Type a URL in the address bar,
or a few words to search the web.</p>
<p><i>Built by panz-08 &amp; nakki-kun.</i></p>
<hr>
<h2>Quick links</h2>
<ul>
"#,
    );
    let quick = [
        ("Wikipedia", "https://en.wikipedia.org/wiki/Main_Page"),
        ("Hacker News", "https://news.ycombinator.com"),
        ("Lobsters", "https://lobste.rs"),
        ("GitHub", "https://github.com"),
        ("Rust Book", "https://doc.rust-lang.org/book/"),
        ("Arch Wiki", "https://wiki.archlinux.org"),
        ("Bing", "https://www.bing.com"),
        ("Panz-08 on GitHub", "https://github.com/Panz-08"),
        ("Nakki-Kun on GitHub", "https://github.com/Nakki-Kun"),
    ];
    for (label, url) in quick {
        html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            html_escape(url),
            html_escape(label)
        ));
    }
    html.push_str("</ul>\n");

    html.push_str("<h2>Bookmarks</h2>\n");
    if bookmarks.is_empty() {
        html.push_str("<p><i>No bookmarks yet. Press <b>m</b> on any page to save one.</i></p>\n");
    } else {
        html.push_str("<ul>\n");
        for b in bookmarks.iter().take(30) {
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>\n",
                html_escape(&b.url),
                html_escape(&b.title)
            ));
        }
        html.push_str("</ul>\n");
    }

    html.push_str(&format!(
        r#"<hr>
<h2>Keys</h2>
<ul>
<li><b>o</b> open a URL &nbsp; <b>t</b> new tab &nbsp; <b>Tab</b> next link &nbsp; <b>Enter</b> follow</li>
<li><b>j</b>/<b>k</b> scroll &nbsp; <b>Space</b> page down &nbsp; <b>g</b>/<b>G</b> top/bottom</li>
<li><b>H</b>/<b>L</b> back/forward &nbsp; <b>r</b> reload &nbsp; <b>/</b> find &nbsp; <b>?</b> help</li>
<li><b>f</b> forms &nbsp; <b>B</b> bookmarks &nbsp; <b>d</b> download &nbsp; <b>u</b> source &nbsp; <b>:</b> commands</li>
<li><b>V</b> visual preview (Firefox screenshot) &nbsp; <b>E</b> Browsh &nbsp; <b>e</b> reader</li>
</ul>
<p><i>Home: {}</i></p>
</body></html>"#,
        html_escape(home_label)
    ));
    html
}
