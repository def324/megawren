use std::{error::Error, fmt};

#[derive(Debug)]
pub struct ExpiredSession;
impl std::fmt::Display for ExpiredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Saved login expired. Sign in again with megawren login.")
    }
}
impl std::error::Error for ExpiredSession {}
#[derive(Debug)]
pub struct UnlockSavedLogin;
impl fmt::Display for UnlockSavedLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Could not unlock saved login.")
    }
}
impl Error for UnlockSavedLogin {}

pub fn message(error: &anyhow::Error) -> String {
    if error.is::<UnlockSavedLogin>() {
        return "Could not unlock saved login.\nTry your local unlock passphrase again.\n\nForgot it? Sign in again:\n  megawren login".into();
    }
    // Only application-owned layout may contain terminal controls/newlines.
    crate::state::display(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlock_failure_has_separate_recovery_instructions() {
        let error = anyhow::anyhow!("synthetic failure").context(UnlockSavedLogin);
        let text = message(&error);
        assert!(text.starts_with("Could not unlock saved login.\n"));
        assert!(text.contains("\n\nForgot it? Sign in again:\n  megawren login"));
        assert!(!text.contains("\\n"));
    }
    #[test]
    fn untrusted_errors_cannot_inject_terminal_controls_or_layout() {
        let text = message(&anyhow::anyhow!("filename\n\x1b[2J"));
        assert!(!text.contains(['\n', '\x1b']));
    }
}
