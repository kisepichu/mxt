//! Reading secrets (the password, the recovery key) from the terminal.

/// Reads a line from the terminal without echo. The trailing newline is
/// removed; nothing else is.
pub fn prompt_secret(prompt: &str) -> std::io::Result<String> {
    rpassword::prompt_password(prompt)
}
