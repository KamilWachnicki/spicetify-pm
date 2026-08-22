//! Terminal output helpers: colored status lines, simple tables, spinners,
//! and the shared interactive pager.

use crate::errors::Result;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

pub fn success(msg: impl AsRef<str>) {
    println!("{} {}", style("✔").green().bold(), msg.as_ref());
}

pub fn info(msg: impl AsRef<str>) {
    println!("{} {}", style("ℹ").blue().bold(), msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    eprintln!("{} {}", style("⚠").yellow().bold(), msg.as_ref());
}

pub fn error(msg: impl std::fmt::Display) {
    eprintln!("{} {msg}", style("✖").red().bold());
}

pub fn style_title(title: &str) -> String {
    style(title).bold().to_string()
}

/// Pre-styled info line (for frames that are printed line-by-line).
pub fn info_line(msg: impl AsRef<str>) -> String {
    format!("{} {}", style("ℹ").blue().bold(), msg.as_ref())
}

/// Rows per interactive page.
pub const PAGE_SIZE: usize = 10;

/// Footer line for pager frames; `action` labels what digits do.
pub fn pager_footer(action: &str) -> String {
    format!("\u{2190}/\u{2192} or p/n page \u{b7} 0-9 {action} \u{b7} g/G first/last \u{b7} q quit")
}

/// Visible length of a string, ignoring ANSI SGR escape sequences so
/// colorized cells align exactly like plain ones.
pub fn visible_len(s: &str) -> usize {
    let mut count = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for esc in chars.by_ref() {
                if esc == 'm' {
                    break;
                }
            }
        } else {
            count += 1;
        }
    }
    count
}

/// How the pager ended.
pub enum PagerExit {
    Quit,
    /// Global index of the item whose page-local digit was pressed.
    Selected(usize),
}

/// Context handed to a pager frame renderer.
pub struct PagerPage {
    pub page: usize,
    pub pages: usize,
    pub start: usize,
    pub end: usize,
    /// Current terminal width, when known — keep rows single-line.
    pub width: Option<usize>,
}

/// Interactive 10-per-page keyboard pager shared by `search` and
/// `snippets list`. Renders one frame in place (clearing the previous one)
/// and returns on quit or digit selection. Caller decides whether to engage
/// (typically: multiple pages && TTY && !json).
pub fn run_pager<F>(
    term: &console::Term,
    total: usize,
    footer: &str,
    render: F,
    current: &mut usize,
) -> Result<PagerExit>
where
    F: Fn(&PagerPage) -> Vec<String>,
{
    // caller-owned so the page survives across action round-trips
    let pages = total.div_ceil(PAGE_SIZE);
    if pages == 0 {
        *current = 0;
    } else {
        *current = (*current).min(pages - 1);
    }
    let render_frame = |page_idx: usize| {
        // full repaint from a blank screen: action status lines and prompts
        // can never stack up between frames
        let _ = term.clear_screen();
        let (start, end) = (
            (page_idx * PAGE_SIZE).min(total),
            ((page_idx + 1) * PAGE_SIZE).min(total),
        );
        let width = term.size_checked().map(|(_, w)| usize::from(w));

        let mut lines = render(&PagerPage {
            page: page_idx,
            pages,
            start,
            end,
            width,
        });
        lines.push(String::new());
        lines.push(info_line(footer));
        for line in &lines {
            println!("{line}");
        }
    };

    render_frame(*current);
    loop {
        match term.read_key()? {
            console::Key::ArrowRight
            | console::Key::Enter
            | console::Key::Char('n' | 'l' | ' ') => {
                if *current + 1 >= pages {
                    continue;
                }
                *current += 1;
            }
            console::Key::ArrowLeft | console::Key::Backspace | console::Key::Char('p' | 'h') => {
                if *current == 0 {
                    continue;
                }
                *current -= 1;
            }
            console::Key::Char('g') => {
                if *current == 0 {
                    continue;
                }
                *current = 0;
            }
            console::Key::Char('G') => {
                if *current + 1 == pages {
                    continue;
                }
                *current = pages - 1;
            }
            console::Key::Char(d @ '0'..='9') => {
                // page-local row number -> global index; digits beyond the
                // last row of a short final page are ignored. Screen is left
                // as-is for the caller's action output; the next frame
                // repaint starts from a blank screen. The page cursor is
                // untouched so callers resume where the user was.
                let idx = *current * PAGE_SIZE + (d as usize - '0' as usize);
                if idx >= total {
                    continue;
                }
                return Ok(PagerExit::Selected(idx));
            }
            console::Key::Char('q') | console::Key::Escape | console::Key::CtrlC => {
                return Ok(PagerExit::Quit);
            }
            _ => continue,
        }
        render_frame(*current);
    }
}

pub fn reminder_apply(apply: bool) {
    if apply {
        crate::commands::apply_hook::run_spicetify_apply();
    } else {
        info(format!(
            "run {} to apply your changes",
            style("spicetify apply").cyan().bold()
        ));
    }
}

/// A minimal aligned table printer.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|s| (*s).to_owned()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    pub fn print(&self) {
        for line in self.to_lines() {
            println!("{line}");
        }
    }

    /// Render the table as one line per row (header included).
    pub fn to_lines(&self) -> Vec<String> {
        let widths = self.column_widths();

        let mut lines = Vec::with_capacity(self.rows.len() + 1);
        let header_line = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, h)| pad(h, widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(style(header_line).bold().to_string());
        for row in &self.rows {
            let line = row
                .iter()
                .enumerate()
                .map(|(i, c)| pad(c, widths[i]))
                .collect::<Vec<_>>()
                .join("  ");
            lines.push(line);
        }
        lines
    }
}

impl Table {
    fn column_widths(&self) -> Vec<usize> {
        let mut widths = self
            .headers
            .iter()
            .map(|h| visible_len(h))
            .collect::<Vec<_>>();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(visible_len(cell));
                }
            }
        }
        widths
    }
}

fn pad(s: &str, width: usize) -> String {
    let len = visible_len(s);
    if len >= width {
        s.to_owned()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// Start a spinner unless we're writing JSON or not attached to a TTY.
pub fn spinner(json: bool, msg: impl Into<String>) -> Option<ProgressBar> {
    if json || !console::Term::stderr().features().is_attended() {
        return None;
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    Some(pb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_len_ignores_ansi_escapes() {
        let styled = "\u{1b}[32m\u{2714} installed\u{1b}[0m";
        assert_eq!(visible_len(styled), visible_len("\u{2714} installed"));
        assert_eq!(visible_len("plain"), 5);
    }

    #[test]
    fn styled_cells_align_like_plain_ones() {
        let mut a = Table::new(&["K"]);
        a.row(vec!["\u{1b}[32m\u{2714} key\u{1b}[0m".to_owned()]);
        let mut b = Table::new(&["K"]);
        b.row(vec!["\u{2714} key".to_owned()]);
        // identical visible width + same padding: colorized cells align
        let (la, lb) = (a.to_lines(), b.to_lines());
        for (styled, plain) in la.iter().zip(lb.iter()) {
            assert_eq!(visible_len(styled), visible_len(plain));
        }
    }
}
