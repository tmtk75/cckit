// Progress reporting for the transcript scans. Rendering is confined to this module so
// the loader and the firing aggregator stay free of UI concerns.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a scan must run before anything is drawn. A warm scan finishes well inside
/// this, so the common case prints nothing rather than flashing a bar for one frame.
const RENDER_DELAY: Duration = Duration::from_millis(250);

/// How often the rendering thread repaints once it has started.
const TICK: Duration = Duration::from_millis(80);

/// How often the rendering thread checks whether the scan is over. Kept well below TICK
/// so `finish()` is not left waiting a whole repaint interval for the thread to notice.
const POLL: Duration = Duration::from_millis(10);

/// The single status line, e.g. `scanning transcripts  1847/3130 (59%)`.
/// An empty corpus counts as complete rather than dividing by zero.
fn progress_line(label: &str, done: usize, total: usize) -> String {
    let percent = (done * 100).checked_div(total).unwrap_or(100);
    format!("{}  {}/{} ({}%)", label, done, total, percent)
}

/// A determinate progress indicator for a parallel scan.
///
/// Counting is lock-free so worker threads only touch an atomic; a single background
/// thread owns the terminal. Nothing is drawn when stderr is not a TTY, which keeps
/// `--json` output and pipelines clean.
pub struct ScanProgress {
    done: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    renderer: Option<JoinHandle<()>>,
}

impl ScanProgress {
    /// Start reporting progress towards `total` items. Returns an inert handle when
    /// stderr is not a terminal or there is nothing to scan.
    pub fn start(label: &str, total: usize) -> Self {
        let done = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        if total == 0 || !std::io::stderr().is_terminal() {
            return Self {
                done,
                stop,
                renderer: None,
            };
        }

        let label = label.to_string();
        let (thread_done, thread_stop) = (Arc::clone(&done), Arc::clone(&stop));
        let renderer = std::thread::spawn(move || {
            let started = Instant::now();
            let mut drawn = false;
            let mut last_paint: Option<Instant> = None;
            while !thread_stop.load(Ordering::Relaxed) {
                if started.elapsed() >= RENDER_DELAY
                    && last_paint.is_none_or(|t| t.elapsed() >= TICK)
                {
                    let line = progress_line(&label, thread_done.load(Ordering::Relaxed), total);
                    let mut err = std::io::stderr();
                    let _ = write!(err, "\r\x1b[2K{}", line);
                    let _ = err.flush();
                    drawn = true;
                    last_paint = Some(Instant::now());
                }
                std::thread::sleep(POLL);
            }
            if drawn {
                let mut err = std::io::stderr();
                let _ = write!(err, "\r\x1b[2K");
                let _ = err.flush();
            }
        });

        Self {
            done,
            stop,
            renderer: Some(renderer),
        }
    }

    /// Record one completed item. Safe to call from any thread.
    pub fn tick(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// Stop rendering and clear the status line.
    pub fn finish(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(renderer) = self.renderer {
            let _ = renderer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_line_shows_done_over_total_with_percent() {
        assert_eq!(
            progress_line("scanning transcripts", 1847, 3130),
            "scanning transcripts  1847/3130 (59%)"
        );
    }

    #[test]
    fn progress_line_reports_100_percent_when_complete() {
        assert_eq!(progress_line("scanning", 20, 20), "scanning  20/20 (100%)");
    }

    #[test]
    fn progress_line_handles_an_empty_corpus_without_dividing_by_zero() {
        assert_eq!(progress_line("scanning", 0, 0), "scanning  0/0 (100%)");
    }
}
