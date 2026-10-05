//! A file this desktop writes what it did to, so a fault can be read afterwards.
//!
//! None of this was readable before. The window has no console on Windows, a
//! phone's log lives on the phone, and anything that went wrong was written to a
//! stderr nobody sees - so "the phone says the computer stopped answering" left
//! nothing at all to look at on this side, which is how an afternoon went into a
//! fault that was a silent host and an older grant.
//!
//! This is the phone's own diagnostics idea, in a file rather than in logcat:
//! one name to look for, small enough to read in an editor.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use napstr_remote_protocol::ClientRequest;

/// The file, one name to look for.
const LOG_FILE: &str = "napstr.log";

/// The name the file that came before is kept under.
const PREVIOUS_LOG_FILE: &str = "log.1";

/// How large the log may grow before what it holds becomes the previous one.
///
/// A megabyte is a few days of ordinary use and a minute of a bad one, which is
/// the minute anybody wants: a log that has to be searched with tools is one
/// that does not get read.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// The log this process is writing, once the data directory is known.
///
/// A process-wide place rather than a value passed down, because the lines worth
/// keeping are written from everywhere - a request being answered, a folder
/// being indexed, a queue being advanced - and threading a logger through all of
/// them would be a change to every one of those call sites instead of a line in
/// each.
static LOG: Mutex<Option<Log>> = Mutex::new(None);

struct Log {
    path: PathBuf,
    file: File,
    written: u64,
}

impl Log {
    /// Open the log, keeping one that has grown too large as the previous file.
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let oversized = fs::metadata(&path)
            .map(|metadata| metadata.len() >= MAX_LOG_BYTES)
            .unwrap_or(false);
        let file = if oversized {
            // Too large before this run even started - after a crash, or after
            // weeks of running. What it holds is kept beside it and the file
            // begins again, rather than being opened and grown further.
            keep_previous(&path);
            File::create(&path)?
        } else {
            // Appended to rather than truncated: a second window, or a restart
            // after a crash, is exactly when the lines before it matter most.
            OpenOptions::new().create(true).append(true).open(&path)?
        };
        let written = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written,
        })
    }

    fn write(&mut self, message: &str) {
        let line = format!("{} {message}\n", now());
        if self.written + line.len() as u64 > MAX_LOG_BYTES {
            self.rotate();
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += line.len() as u64;
        }
    }

    /// Put what has been written aside and start the file again.
    ///
    /// Copied out and started over rather than renamed, because Windows will not
    /// rename a file that is open - and the handle writing this log is, by
    /// definition, open. Emptied by opening it again rather than by shortening
    /// it, for the same platform's other reason: a file opened to be appended to
    /// is not a file opened to be written over, and `set_len` on one of those is
    /// refused. The old handle is dropped as the new one arrives, which is what
    /// lets the name be reused at all.
    ///
    /// The awkward case is not hypothetical: this application stays open for
    /// weeks, so the cap has to be enforced while it runs.
    fn rotate(&mut self) {
        keep_previous(&self.path);
        if let Ok(file) = File::create(&self.path) {
            self.file = file;
            self.written = 0;
        }
    }
}

/// Keep what the log holds now beside it, under the name the previous one has.
fn keep_previous(path: &Path) {
    if let Ok(bytes) = fs::read(path) {
        let _ = fs::write(path.with_extension(PREVIOUS_LOG_FILE), bytes);
    }
}

/// The time a line was written, to the millisecond.
///
/// Milliseconds because half of what this log is asked is how long something
/// took, and a whole second cannot answer that.
fn now() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// Start writing, in the directory this application keeps its own things in.
///
/// Called once, as soon as that directory is known - before the services, so
/// that failing to start one is in the log that explains it. A desktop that
/// cannot write a log still works: every call after a failure does nothing.
pub fn start(app_data: &Path) {
    let path = app_data.join(LOG_FILE);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut guard = lock();
    match Log::open(path) {
        Ok(mut log) => {
            log.write(&format!("--- Napstr {} started ---", env!("CARGO_PKG_VERSION")));
            *guard = Some(log);
        }
        Err(error) => {
            *guard = None;
            eprintln!("Napstr could not write its log: {error}");
        }
    }
}

/// Write one line, if there is anywhere to write it.
///
/// Also to stderr, which is where these lines came from and where a run from a
/// terminal still wants them.
pub fn note(message: &str) {
    eprintln!("{message}");
    let mut guard = lock();
    if let Some(log) = guard.as_mut() {
        log.write(message);
    }
}

/// The lock, whether or not a panic somewhere left it poisoned.
///
/// A line is never worth failing over, and never worth failing twice: a panic in
/// one task must not stop every later line from being written, when the lines
/// after a panic are the ones worth having.
fn lock() -> MutexGuard<'static, Option<Log>> {
    match LOG.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A device, as much of its key as a line needs.
///
/// The same eight characters and the same ellipsis the phone's own log uses, so
/// the two halves of one conversation can be lined up by eye. A name that is
/// already short is left alone: an ellipsis says something was cut off, and
/// saying that about a whole name is a small lie in a file read for its facts.
pub fn device(endpoint_id: &str) -> String {
    if endpoint_id.chars().count() <= 8 {
        return endpoint_id.to_string();
    }
    format!("{:.8}…", endpoint_id)
}

/// What a request is called on the wire.
///
/// Read out of the frame rather than matched here, so this file and the phone's
/// log use one word for one thing: the word neither side can disagree about is
/// the tag serde already writes.
pub fn request_kind(request: &ClientRequest) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| value.get("type")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

/// How long something took, for a line that says how long.
pub fn millis(since: std::time::Instant) -> String {
    format!("{}ms", since.elapsed().as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("napstr-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A line has to be in the file, not in a buffer that dies with the process.
    ///
    /// This is the whole point of the file: the interesting case is the run that
    /// ended badly, and a log that is written out on a clean exit is a log that
    /// is missing exactly the run it was added for.
    #[test]
    fn a_line_written_is_a_line_in_the_file() {
        let root = directory("written");
        let path = root.join(LOG_FILE);
        let mut log = Log::open(path.clone()).unwrap();
        log.write("the first thing that happened");
        log.write("and then this");

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("the first thing that happened"));
        assert!(contents.contains("and then this"));
        // One line per line, in order, with the time in front of each.
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("20"), "a line starts with its time");
        assert!(lines[0].ends_with("the first thing that happened"));
        let _ = fs::remove_dir_all(root);
    }

    /// A log that is full becomes the previous one rather than growing without
    /// end or being thrown away.
    ///
    /// The minute before a fault is the minute worth reading, and discarding it
    /// at a cap would throw away precisely that.
    #[test]
    fn a_full_log_becomes_the_previous_one() {
        let root = directory("rotation");
        let path = root.join(LOG_FILE);
        fs::write(&path, "x".repeat(MAX_LOG_BYTES as usize)).unwrap();

        let mut log = Log::open(path.clone()).unwrap();
        log.write("what happened after the restart");

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("what happened after the restart"));
        assert!(!contents.contains("xxx"), "the new file starts empty");
        let previous = fs::read_to_string(root.join("napstr.log.1")).unwrap();
        assert_eq!(previous.len(), MAX_LOG_BYTES as usize);
        let _ = fs::remove_dir_all(root);
    }

    /// A desktop that stays open for weeks writes past the cap without being
    /// restarted, so the cap is checked on the way in, not only on the way up.
    #[test]
    fn a_log_that_fills_while_it_is_being_written_is_rotated() {
        let root = directory("growing");
        let path = root.join(LOG_FILE);
        let mut log = Log::open(path.clone()).unwrap();

        for index in 0..20_000 {
            log.write(&format!("ordinary line {index} of a long run"));
        }

        assert!(root.join("napstr.log.1").exists());
        assert!(
            fs::metadata(&path).unwrap().len() <= MAX_LOG_BYTES,
            "the file in use stays under the cap"
        );
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("ordinary line"), "the latest lines are here");
        let _ = fs::remove_dir_all(root);
    }

    /// The word a request is logged under is the word the wire uses.
    ///
    /// The other half of the same conversation is logged on the phone, and the
    /// two are only readable as one story if they call things the same thing -
    /// which the tag serde already writes is the one naming both sides share.
    #[test]
    fn a_request_is_named_the_way_the_wire_names_it() {
        assert_eq!(request_kind(&ClientRequest::Status), "status");
        assert_eq!(request_kind(&ClientRequest::Likes), "likes");
        assert_eq!(
            request_kind(&ClientRequest::FetchAudio {
                file_id: "ab".into()
            }),
            "fetchAudio"
        );
        assert_eq!(
            request_kind(&ClientRequest::IdentityChallenge),
            "identityChallenge"
        );
    }

    /// The name a device is spoken of by is short, and the same eight characters
    /// the phone uses for the same computer.
    #[test]
    fn a_device_is_named_by_a_key_in_short() {
        assert_eq!(device("0f43fbc7a7ccdff6d1f2"), "0f43fbc7…");
        assert_eq!(device("short"), "short");
    }
}
