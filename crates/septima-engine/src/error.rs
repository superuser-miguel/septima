use std::fmt;
use std::io;

/// Errors from driving the `7zz` CLI.
#[derive(Debug)]
pub enum EngineError {
    /// The `7zz` process could not be spawned (missing binary, permissions).
    Spawn(io::Error),
    /// The archive (or its header) is encrypted; a password is required to list it.
    PasswordRequired,
    /// `7zz` exited non-zero for another reason.
    SevenZip { code: Option<i32>, stderr: String },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Spawn(e) => write!(f, "failed to run 7zz: {e}"),
            EngineError::PasswordRequired => write!(f, "a password is required to read this archive"),
            EngineError::SevenZip { code, stderr } => match code {
                Some(c) => write!(f, "7zz exited with code {c}: {}", stderr.trim()),
                None => write!(f, "7zz was terminated: {}", stderr.trim()),
            },
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Spawn(e) => Some(e),
            _ => None,
        }
    }
}
