use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::thread;
use indicatif::{ProgressBar, ProgressStyle};
use std::thread::JoinHandle;

pub enum ProgressBarMessage {
    Increment,
    Println(String)
}

pub fn progress_bar(count: usize, prefix: String) -> (JoinHandle<()>, Sender<ProgressBarMessage>) {
    let (sender, receiver) = mpsc::channel();
    let pb_thread = thread::spawn(move || {
        let pb = ProgressBar::new(count as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{prefix} {wide_bar} {pos}/{len} [{elapsed_precise}]")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_prefix(&prefix);
        pb.enable_steady_tick(500);
        pb.tick();
        for _ in 0..count {
            match receiver.recv() {
                Ok(ProgressBarMessage::Increment) => pb.inc(1),
                Ok(ProgressBarMessage::Println(s)) => pb.println(s),
                Err(_) => {
                    pb.println("Unexpected progress bar channel breakage.");
                    break;
                },
            }
        }
        pb.finish();
    });

    (pb_thread, sender)
}