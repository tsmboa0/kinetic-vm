//! The KineticVM mark printed once at the start of an interactive command.
//!
//! The loading bar runs only while a caller is waiting on real work. A
//! non-interactive stderr (a service journal, a pipe) gets neither.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PURPLE: &str = "38;2;168;85;247";
const WHITE: &str = "97";
const BAR_WIDTH: usize = 18;
const BAR_SLIDER: usize = 4;

/// True when the environment allows ANSI color.
pub fn ansi_color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
}

/// Write the mark and a blank line.
pub fn write_mark<W: Write>(mut out: W, color: bool) -> io::Result<()> {
    for line in render_mark(color) {
        writeln!(out, "{line}")?;
    }
    writeln!(out)
}

pub fn render_mark(color: bool) -> [String; 5] {
    let kinetic = compose("KINETIC");
    let vm = compose("VM");
    std::array::from_fn(|row| {
        format!(
            "  {}  {}",
            paint(&kinetic[row], color, PURPLE),
            paint(&vm[row], color, WHITE)
        )
    })
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

fn compose(word: &str) -> [String; 5] {
    let mut rows = std::array::from_fn::<String, 5, _>(|_| String::new());
    for (index, ch) in word.chars().enumerate() {
        if index > 0 {
            for row in &mut rows {
                row.push(' ');
            }
        }
        for (row_index, glyph_row) in glyph(ch).into_iter().enumerate() {
            rows[row_index].push_str(glyph_row);
        }
    }
    rows
}

fn glyph(ch: char) -> [&'static str; 5] {
    match ch {
        'K' => ["█   █", "█  █ ", "███  ", "█  █ ", "█   █"],
        'I' => ["█████", "  █  ", "  █  ", "  █  ", "█████"],
        'N' => ["█   █", "██  █", "█ █ █", "█  ██", "█   █"],
        'E' => ["█████", "█    ", "███  ", "█    ", "█████"],
        'T' => ["█████", "  █  ", "  █  ", "  █  ", "  █  "],
        'C' => ["█████", "█    ", "█    ", "█    ", "█████"],
        'V' => ["█   █", "█   █", "█   █", " █ █ ", "  █  "],
        'M' => ["█   █", "██ ██", "█ █ █", "█   █", "█   █"],
        _ => ["     ", "     ", "     ", "     ", "     "],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_row_is_five_cells() {
        for ch in ['K', 'I', 'N', 'E', 'T', 'C', 'V', 'M'] {
            for row in glyph(ch) {
                assert_eq!(row.chars().count(), 5, "{ch}");
            }
        }
    }

    #[test]
    fn the_plain_mark_aligns_and_carries_no_color() {
        let mark = render_mark(false);
        let width = mark[0].chars().count();
        assert!(width <= 80, "{width}");
        for line in &mark {
            assert_eq!(line.chars().count(), width);
            assert!(!line.contains('\u{1b}'));
        }
        assert!(mark[0].contains('█'));
        assert!(mark[2].contains("███"));
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
