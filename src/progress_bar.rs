use indicatif::{ProgressBar, ProgressStyle};
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::thread;
use std::thread::JoinHandle;

pub enum ProgressBarMessage {
    Increment,
    SetProgress(usize, usize),
    Done,
    Println(String),
}

/// Creates a progress bar thread that can receive `ProgressBarMessage`s
///
/// A message of `ProgressBarMessage::Done` must be sent before calling the `JoinHandle`,
/// otherwise the thread will hang indefinitely.
///
/// Sending `ProgressBarMessage::Increment` more times than the `count` will not cause any issues.
pub fn progress_bar(
    count: Option<usize>,
    prefix: String,
) -> (JoinHandle<()>, Sender<ProgressBarMessage>) {
    let (sender, receiver) = mpsc::channel();
    let pb_thread = thread::spawn(move || {
        let pb = if let Some(count) = count {
            ProgressBar::new(count as u64)
        } else {
            ProgressBar::new(1)
        };
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {pos}/{len} [{elapsed_precise}]")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_prefix(prefix);
        pb.enable_steady_tick(500);
        pb.tick();
        loop {
            match receiver.recv() {
                Ok(ProgressBarMessage::Increment) => pb.inc(1),
                Ok(ProgressBarMessage::Println(s)) => pb.println(s),
                Ok(ProgressBarMessage::SetProgress(current, total)) => {
                    pb.set_position(current as u64);
                    pb.set_length(total as u64);
                }
                Ok(ProgressBarMessage::Done) => break,
                Err(_) => {
                    pb.println("Unexpected progress bar channel breakage.");
                    break;
                }
            }
        }
        pb.finish();
    });

    (pb_thread, sender)
}
