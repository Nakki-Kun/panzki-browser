use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::imageops::FilterType;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use url::Url;

/// A rendered screenshot, split into terminal lines of half-block cells.
pub struct VisualPage {
    pub lines: Vec<Line<'static>>,
    pub width: usize,
    pub height: usize,
}

/// Capture a page with headless Firefox and convert the screenshot into
/// coloured half-block cells that ratatui can draw.
pub fn capture(url: &Url, cols: u16, rows: u16) -> Result<VisualPage> {
    if cols < 20 || rows < 6 {
        bail!("the terminal is too small for a visual preview");
    }

    let home = dirs::home_dir().context("could not find the home directory")?;
    let (profile, shot) = paths(&home);
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile).context("creating the Firefox profile directory")?;
    let _ = std::fs::remove_file(&shot);

    // A tall viewport gives us more of the page; the result is scrollable.
    let pixel_w = 1200u32;
    let pixel_h = 2400u32;

    let mut child = Command::new("firefox")
        .arg("--headless")
        .arg("--new-instance")
        .arg("--no-remote")
        .arg("--profile")
        .arg(&profile)
        .arg("--screenshot")
        .arg(&shot)
        .arg(format!("--window-size={pixel_w},{pixel_h}"))
        .arg(url.as_str())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not launch Firefox — install it, or use Browsh with E")?;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match child.try_wait()? {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            None => std::thread::sleep(Duration::from_millis(120)),
        }
    }

    if !shot.exists() {
        let _ = std::fs::remove_dir_all(&profile);
        bail!("Firefox did not produce a screenshot");
    }

    let image = image::open(&shot)
        .context("reading the Firefox screenshot")?
        .to_rgb8();
    let _ = std::fs::remove_file(&shot);
    let _ = std::fs::remove_dir_all(&profile);

    let target_w = cols as u32;
    let target_h =
        ((image.height() as f64 / image.width() as f64) * target_w as f64).round() as u32;
    let target_h = (target_h.max(2) & !1).max(2);
    let resized = image::imageops::resize(&image, target_w, target_h, FilterType::Triangle);

    let mut lines = Vec::with_capacity((target_h / 2) as usize);
    for y in (0..target_h).step_by(2) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut top = (0u8, 0u8, 0u8);
        let mut bottom = (0u8, 0u8, 0u8);

        for x in 0..target_w {
            let t = resized.get_pixel(x, y);
            let b = resized.get_pixel(x, y + 1);
            let t = (t[0], t[1], t[2]);
            let b = (b[0], b[1], b[2]);
            if run.is_empty() {
                top = t;
                bottom = b;
                run.push('▀');
            } else if t == top && b == bottom {
                run.push('▀');
            } else {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(rgb(top)).bg(rgb(bottom)),
                ));
                top = t;
                bottom = b;
                run.push('▀');
            }
        }
        if !run.is_empty() {
            spans.push(Span::styled(
                run,
                Style::default().fg(rgb(top)).bg(rgb(bottom)),
            ));
        }
        lines.push(Line::from(spans));
    }

    let height = lines.len();
    Ok(VisualPage {
        lines,
        width: target_w as usize,
        height,
    })
}

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Snap Firefox can only access its own home, so keep the profile and
/// screenshot inside `~/snap/firefox/common` when the snap is installed.
fn paths(home: &Path) -> (PathBuf, PathBuf) {
    let snap = home.join("snap/firefox/common");
    if snap.is_dir() {
        let base = snap.join(".mozilla/firefox");
        (base.join("panzki-visual"), snap.join("panzki-visual.png"))
    } else {
        let base = home.join(".cache/panzki");
        (base.join("visual-profile"), base.join("visual.png"))
    }
}
