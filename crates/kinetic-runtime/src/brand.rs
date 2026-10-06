//! The KineticVM mark printed once at the start of an interactive command.
//!
//! The loading bar runs only while a caller is waiting on real work. A
//! non-interactive stderr (a service journal, a pipe) gets neither.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PURPLE: &str = "38;2;168;85;247";
const WHITE: &str = "97";
const DIM: &str = "38;2;161;161;170";
const BAR_WIDTH: usize = 18;
const BAR_SLIDER: usize = 4;

/// How long an interactive quickstart leaves the mark up before the prompts.
pub const MARK_HOLD: std::time::Duration = std::time::Duration::from_secs(2);

/// True when the environment allows ANSI color.
pub fn ansi_color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
}

/// Write a blank line, the mark, and a blank line.
pub fn write_mark<W: Write>(mut out: W, color: bool) -> io::Result<()> {
    writeln!(out)?;
    for line in render_mark(color) {
        writeln!(out, "{line}")?;
    }
    writeln!(out)
}

/// Leave the mark on screen before an interactive prompt takes over.
pub async fn hold_mark() {
    tokio::time::sleep(MARK_HOLD).await;
}

pub fn render_mark(color: bool) -> Vec<String> {
    let kinetic = compose("KINETIC");
    let vm = compose("VM");
    let body: Vec<String> = (0..kinetic.len())
        .map(|row| format!("  {}   {}", kinetic[row], vm[row]))
        .collect();
    let tagline = crate::i18n::get_required_cli_string("cli-brand-tagline");
    let width = body
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(tagline.chars().count());
    let mut lines: Vec<String> = body
        .iter()
        .enumerate()
        .map(|(row, plain)| {
            let (left, right) = side_pad(plain.chars().count(), width);
            format!(
                "{left}  {}   {}{right}",
                paint(&kinetic[row], color, PURPLE),
                paint(&vm[row], color, WHITE),
            )
        })
        .collect();
    lines.push(paint(&"─".repeat(width), color, DIM));
    let (left, right) = side_pad(tagline.chars().count(), width);
    lines.push(format!("{left}{}{right}", paint(&tagline, color, DIM)));
    lines
}

fn side_pad(len: usize, width: usize) -> (String, String) {
    let extra = width.saturating_sub(len);
    let left = extra / 2;
    (" ".repeat(left), " ".repeat(extra - left))
}

/// One frame of the indeterminate bar. `tick` slides the purple block.
pub fn render_bar(tick: usize, label: &str, color: bool) -> String {
    let mut cells = vec!['░'; BAR_WIDTH];
    for step in 0..BAR_SLIDER {
        cells[(tick.wrapping_add(step)) % BAR_WIDTH] = '█';
    }
    let track: String = cells.into_iter().collect();
    if color {
        format!("\x1b[{PURPLE}m{track}\x1b[0m  \x1b[{WHITE}m{label}\x1b[0m")
    } else {
        format!("{track}  {label}")
    }
}

pub fn render_bar_done(color: bool) -> String {
    let track = "█".repeat(BAR_WIDTH);
    if color {
        format!("\x1b[{PURPLE}m{track}\x1b[0m")
    } else {
        track
    }
}

/// Redraws a bar on stderr until [`ConnectBar::finish`].
///
/// `animate` is false for a service journal or a pipe. Finish is then immediate
/// and writes nothing.
pub struct ConnectBar {
    stop: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ConnectBar {
    pub fn start(label: impl Into<String>, animate: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if !animate {
            return Self { stop, task: None };
        }
        let flag = Arc::clone(&stop);
        let label = label.into();
        let task = kinetic_spawn::spawn!(async move {
            let color = ansi_color_enabled();
            let mut tick = 0usize;
            let mut err = io::stderr();
            while !flag.load(Ordering::Relaxed) {
                let frame = render_bar(tick, &label, color);
                let _ = write!(err, "\r\x1b[2K{frame}");
                let _ = err.flush();
                tick = tick.wrapping_add(1);
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
            let _ = write!(err, "\r\x1b[2K{}\n", render_bar_done(color));
            let _ = err.flush();
        });
        Self {
            stop,
            task: Some(task),
        }
    }

    pub async fn finish(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ConnectBar {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn paint(text: &str, color: bool, code: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn compose(word: &str) -> [String; 6] {
    let mut rows = std::array::from_fn::<String, 6, _>(|_| String::new());
    for ch in word.chars() {
        for (row_index, glyph_row) in glyph(ch).into_iter().enumerate() {
            rows[row_index].push_str(glyph_row);
        }
    }
    rows
}

fn glyph(ch: char) -> [&'static str; 6] {
    match ch {
        'K' => [
            "██╗  ██╗",
            "██║ ██╔╝",
            "█████╔╝ ",
            "██╔═██╗ ",
            "██║  ██╗",
            "╚═╝  ╚═╝",
        ],
        'I' => ["██╗", "██║", "██║", "██║", "██║", "╚═╝"],
        'N' => [
            "███╗   ██╗",
            "████╗  ██║",
            "██╔██╗ ██║",
            "██║╚██╗██║",
            "██║ ╚████║",
            "╚═╝  ╚═══╝",
        ],
        'E' => [
            "███████╗",
            "██╔════╝",
            "█████╗  ",
            "██╔══╝  ",
            "███████╗",
            "╚══════╝",
        ],
        'T' => [
            "████████╗ ",
            "╚══██╔══╝ ",
            "   ██║    ",
            "   ██║    ",
            "   ██║    ",
            "   ╚═╝    ",
        ],
        'C' => [
            " ██████╗",
            "██╔════╝",
            "██║     ",
            "██║     ",
            "╚██████╗",
            " ╚═════╝",
        ],
        'V' => [
            "██╗   ██╗",
            "██║   ██║",
            "██║   ██║",
            "╚██╗ ██╔╝",
            " ╚████╔╝ ",
            "  ╚═══╝  ",
        ],
        'M' => [
            "███╗   ███╗",
            "████╗ ████║",
            "██╔████╔██║",
            "██║╚██╔╝██║",
            "██║ ╚═╝ ██║",
            "╚═╝     ╚═╝",
        ],
        _ => [" ", " ", " ", " ", " ", " "],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_written_mark_leaves_a_blank_line_above_the_letters() {
        let mut buf = Vec::new();
        write_mark(&mut buf, false).expect("mark");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.starts_with("\n  "));
        assert!(!text.starts_with("\n\n"));
        assert!(text.contains('█'));
    }

    #[test]
    fn every_glyph_row_shares_its_letter_width() {
        for ch in ['K', 'I', 'N', 'E', 'T', 'C', 'V', 'M'] {
            let rows = glyph(ch);
            let width = rows[0].chars().count();
            for row in rows {
                assert_eq!(row.chars().count(), width, "{ch}");
            }
        }
    }

    #[test]
    fn the_plain_mark_aligns_and_carries_no_color() {
        let mark = render_mark(false);
        let width = mark[0].chars().count();
        assert!(width <= 80, "{width}");
        for line in &mark {
            assert_eq!(line.chars().count(), width, "{line}");
            assert!(!line.contains('\u{1b}'));
        }
        assert!(mark[0].contains('█'));
        assert!(mark.iter().any(|line| line.contains('╚')));
        assert!(
            mark.last()
                .is_some_and(|line| line.contains("physical device"))
        );
        assert_eq!(MARK_HOLD, std::time::Duration::from_secs(2));
    }

    #[test]
    fn the_color_mark_paints_kinetic_purple_and_vm_white() {
        let mark = render_mark(true);
        let line = &mark[0];
        let purple = line.find("\x1b[38;2;168;85;247m").expect("purple");
        let white = line.find("\x1b[97m").expect("white");
        assert!(purple < white);
        assert!(line.matches("\x1b[0m").count() >= 2);
    }

    #[test]
    fn the_bar_slides_and_the_done_bar_is_full() {
        let first = render_bar(0, "Connecting", false);
        let later = render_bar(3, "Connecting", false);
        assert_ne!(first, later);
        assert!(first.ends_with("  Connecting"));
        assert_eq!(render_bar_done(false).chars().count(), BAR_WIDTH);
        assert!(render_bar_done(false).chars().all(|ch| ch == '█'));
        assert!(render_bar(0, "Connecting", true).contains("\x1b[38;2;168;85;247m"));
        assert!(!render_bar(0, "Connecting", false).contains('\u{1b}'));
    }

    #[tokio::test]
    async fn an_idle_bar_finishes_without_a_task() {
        let bar = ConnectBar::start("Connecting", false);
        assert!(bar.task.is_none());
        bar.finish().await;
    }
}
