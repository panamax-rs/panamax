use console::{pad_str, style};

pub fn current_step_prefix(step: usize, steps: usize) -> String {
    style(format!("[{}/{}]", step, steps)).bold().to_string()
}

pub fn padded_prefix_message(step: usize, steps: usize, msg: &str) -> String {
    pad_str(
        &format!("{} {}...", current_step_prefix(step, steps), msg),
        34,
        console::Alignment::Left,
        None,
    )
    .to_string()
}
