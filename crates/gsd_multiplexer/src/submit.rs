//! Task submission to the multiplexer daemon.

use crate::constants::SOCKET_NAME;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Submit a task to the multiplexer and wait for the result.
///
/// Connects to the daemon's Unix socket, sends the task, and blocks
/// until the result is available.
pub fn submit(root: impl AsRef<Path>, input: &str) -> io::Result<String> {
    let socket_path = root.as_ref().join(SOCKET_NAME);

    let mut stream = UnixStream::connect(&socket_path)?;
    stream.set_read_timeout(None)?;

    writeln!(stream, "{}", input.len())?;
    stream.write_all(input.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);

    let mut len_line = String::new();
    reader.read_line(&mut len_line)?;
    let len: usize = len_line
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut output = vec![0u8; len];
    reader.read_exact(&mut output)?;

    String::from_utf8(output).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
