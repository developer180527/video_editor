use std::io::{Read, Write};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("could not start {0}: {1}")]
    Spawn(String, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A running child process with a byte pipe each way. The add-on protocol
/// runs over these pipes; the host never shares memory with the child except
/// through explicitly created shared buffers.
pub trait ChildProcess: Send {
    fn stdin(&mut self) -> &mut (dyn Write + Send);
    fn stdout(&mut self) -> &mut (dyn Read + Send);
    /// `Some(code)` once it has exited.
    fn try_wait(&mut self) -> Result<Option<i32>, ProcessError>;
    fn kill(&mut self) -> Result<(), ProcessError>;
}

/// Only on platforms where [`crate::Capabilities::processes`] is true.
pub trait ProcessHost: Send + Sync {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Box<dyn ChildProcess>, ProcessError>;
}
