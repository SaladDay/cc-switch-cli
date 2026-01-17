use std::io::{self, IsTerminal, Write};
use std::sync::Once;

static DISABLE_BRACKETED_PASTE: Once = Once::new();

/// Disables bracketed paste mode to work around inquire dropping paste events.
///
/// When bracketed paste mode is enabled (common in zsh/fish, tmux/zellij, and some terminals),
/// paste events are sent as `Event::Paste(String)` by crossterm. However, inquire's event
/// handler only processes `Event::Key(...)` and drops all other events, causing paste to
/// appear broken.
///
/// This function sends the ANSI escape sequence `CSI ?2004 l` to disable bracketed paste mode,
/// causing the terminal to send paste content as regular character events that inquire can handle.
///
/// **Side effects:**
/// - This changes terminal state and may affect bracketed paste behavior after the program exits.
/// - Most shells (zsh/fish) will re-enable it on the next prompt, but this is not guaranteed.
///
/// **Implementation notes:**
/// - Uses `Once` to ensure the sequence is only sent once per process.
/// - This is a best-effort operation that silently fails if stderr is not a terminal.
/// - On Windows, the ANSI sequence may appear as garbage in legacy consoles that don't support VT.
pub fn disable_bracketed_paste_mode_best_effort() {
    DISABLE_BRACKETED_PASTE.call_once(|| {
        if !io::stderr().is_terminal() {
            return;
        }

        let mut stderr = io::stderr();
        // CSI ?2004 l - Disable bracketed paste mode
        if let Err(e) = stderr.write_all(b"\x1b[?2004l") {
            log::debug!("Failed to disable bracketed paste mode: {}", e);
            return;
        }
        if let Err(e) = stderr.flush() {
            log::debug!(
                "Failed to flush stderr after disabling bracketed paste: {}",
                e
            );
        }
    });
}
