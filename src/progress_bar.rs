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

pub fn progress_bar(count: Option<usize>, prefix: String) -> (JoinHandle<()>, Sender<ProgressBarMessage>) {
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
        pb.set_prefix(&prefix);
        pb.enable_steady_tick(500);
        pb.tick();
        let mut progress = 0;
        loop {
            if let Some(count) = count {
                if count > 0 && progress == count {
                    break;
                } else {
                    progress += 1;
                }
            }
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
