use std::io::{self, Write};
use std::thread;
use std::time::Duration;

/// Shooting star with rainbow comet trail, left-to-right. Blocks for ~1.2s.
/// Skips silently if stdout isn't a tty (piped output, launchd logs).
pub fn shooting_star() {
    if !stdout_is_tty() {
        return;
    }

    let width = term_width().unwrap_or(80).max(30);
    let trail_colors: [u8; 8] = [196, 202, 208, 220, 46, 51, 27, 129];
    let trail_len = trail_colors.len();
    let head = '✦';
    let trail_chars = ['✧', '⋆', '·', '·', '·', '·', '·', ' '];

    // Hide cursor for the duration of the animation.
    print!("\x1b[?25l");
    println!();

    let total = width as usize + trail_len + 2;
    for step in 0..total {
        let head_pos = step as i32;

        let mut line = String::new();
        for col in 0..width as i32 {
            if col == head_pos {
                line.push_str(&format!("\x1b[1;38;5;231m{}\x1b[0m", head));
            } else {
                let offset = head_pos - col;
                if offset > 0 && (offset as usize) <= trail_len {
                    let idx = (offset as usize) - 1;
                    let color = trail_colors[idx];
                    let glyph = trail_chars[idx];
                    line.push_str(&format!("\x1b[1;38;5;{}m{}\x1b[0m", color, glyph));
                } else {
                    line.push(' ');
                }
            }
        }

        print!("\r{}", line);
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_millis(18));
    }

    // Clear the line and restore the cursor.
    print!("\r");
    for _ in 0..width {
        print!(" ");
    }
    print!("\r\x1b[?25h");
    let _ = io::stdout().flush();
    println!();
}

fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDOUT_FILENO) == 1 }
}

fn term_width() -> Option<u16> {
    use libc::{ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ};
    let mut ws: winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut ws) };
    if rc == 0 && ws.ws_col > 0 {
        Some(ws.ws_col)
    } else {
        None
    }
}
