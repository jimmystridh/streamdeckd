//! Structured logging with size-based rotation.
//!
//! Logs go to a bounded set of files under `~/Library/Logs/streamdeckd` and, when
//! run in the foreground, to stderr. The level can be changed at runtime through
//! the control socket without restarting.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, EnvFilter};

/// Rotate once a log file passes this size.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Keep this many rotated files, newest first.
const KEEP: usize = 5;

/// A writer that rotates `streamdeckd.log` by size and keeps `KEEP` old files.
#[derive(Debug)]
pub struct RotatingWriter {
    directory: PathBuf,
    name: String,
    file: Mutex<Option<File>>,
    written: Mutex<u64>,
}

impl RotatingWriter {
    pub fn new(directory: impl Into<PathBuf>, name: impl Into<String>) -> io::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        let writer = Self {
            directory,
            name: name.into(),
            file: Mutex::new(None),
            written: Mutex::new(0),
        };
        writer.open()?;
        Ok(writer)
    }

    fn path(&self) -> PathBuf {
        self.directory.join(&self.name)
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("{}.{index}", self.name))
    }

    fn open(&self) -> io::Result<()> {
        let path = self.path();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        *self.file.lock().expect("log file lock") = Some(file);
        *self.written.lock().expect("log size lock") = size;
        Ok(())
    }

    /// Shifts `name.4` → dropped, `name.3` → `name.4`, …, `name` → `name.1`.
    fn rotate(&self) -> io::Result<()> {
        *self.file.lock().expect("log file lock") = None;

        let oldest = self.rotated_path(KEEP);
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for index in (1..KEEP).rev() {
            let from = self.rotated_path(index);
            if from.exists() {
                fs::rename(&from, self.rotated_path(index + 1))?;
            }
        }
        if self.path().exists() {
            fs::rename(self.path(), self.rotated_path(1))?;
        }
        self.open()
    }

    /// Number of files this writer currently owns, for the doctor check.
    pub fn file_count(&self) -> usize {
        (0..=KEEP)
            .filter(|index| {
                if *index == 0 {
                    self.path().exists()
                } else {
                    self.rotated_path(*index).exists()
                }
            })
            .count()
    }
}

impl Write for &RotatingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let should_rotate = {
            let written = self.written.lock().expect("log size lock");
            *written + buffer.len() as u64 > MAX_BYTES
        };
        if should_rotate {
            self.rotate()?;
        }

        let mut file = self.file.lock().expect("log file lock");
        let Some(file) = file.as_mut() else {
            return Ok(buffer.len());
        };
        let written = file.write(buffer)?;
        *self.written.lock().expect("log size lock") += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut file = self.file.lock().expect("log file lock");
        match file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Lets the control socket raise or lower the level while running.
#[derive(Clone)]
pub struct LevelControl {
    handle: reload::Handle<EnvFilter, tracing_subscriber::Registry>,
    current: Arc<Mutex<String>>,
}

/// The levels the CLI accepts. `EnvFilter` would otherwise treat an unknown word
/// as a target directive and silently disable all output.
const LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

impl LevelControl {
    pub fn set(&self, level: &str) -> Result<(), String> {
        let normalized = level.trim().to_ascii_lowercase();
        if !LEVELS.contains(&normalized.as_str()) {
            return Err(format!(
                "unknown log level `{level}`; expected one of {}",
                LEVELS.join(", ")
            ));
        }
        let filter = EnvFilter::try_new(&normalized).map_err(|error| error.to_string())?;
        self.handle
            .reload(filter)
            .map_err(|error| error.to_string())?;
        *self.current.lock().expect("level lock") = normalized;
        Ok(())
    }

    pub fn current(&self) -> String {
        self.current.lock().expect("level lock").clone()
    }
}

/// Installs the subscriber. `foreground` also logs to stderr.
pub fn init(
    directory: &Path,
    default_level: &str,
    foreground: bool,
) -> anyhow::Result<(LevelControl, Arc<RotatingWriter>)> {
    let writer = Arc::new(RotatingWriter::new(directory, "streamdeckd.log")?);
    let (filter, handle) = reload::Layer::new(EnvFilter::try_new(default_level)?);

    let file_writer = Arc::clone(&writer);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(move || FileWriter(Arc::clone(&file_writer)));

    let stderr_layer = foreground.then(|| {
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_writer(io::stderr)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        // A second init in the same process (tests) is not an error worth failing on.
        .ok();

    Ok((
        LevelControl {
            handle,
            current: Arc::new(Mutex::new(default_level.to_string())),
        },
        writer,
    ))
}

/// Adapts the shared rotating writer to `MakeWriter`'s per-event writer.
struct FileWriter(Arc<RotatingWriter>);

impl Write for FileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_writer_creates_its_directory_and_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("nested");
        let writer = RotatingWriter::new(&path, "test.log").expect("writer");

        assert!(path.join("test.log").exists());
        assert_eq!(writer.file_count(), 1);
    }

    #[test]
    fn writes_land_in_the_current_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let writer = RotatingWriter::new(directory.path(), "test.log").expect("writer");

        (&writer).write_all(b"first line\n").expect("write");
        (&writer).flush().expect("flush");

        let contents = fs::read_to_string(directory.path().join("test.log")).expect("read");
        assert_eq!(contents, "first line\n");
    }

    #[test]
    fn passing_the_size_limit_rotates_and_keeps_a_bounded_number_of_files() {
        let directory = tempfile::tempdir().expect("temp dir");
        let writer = RotatingWriter::new(directory.path(), "test.log").expect("writer");
        let chunk = vec![b'x'; 1024];

        // Enough writes to rotate several times past the 4 MiB limit.
        for _ in 0..(MAX_BYTES / 1024 * (KEEP as u64 + 2)) {
            (&writer).write_all(&chunk).expect("write");
        }
        (&writer).flush().expect("flush");

        assert!(writer.file_count() <= KEEP + 1, "{}", writer.file_count());
        assert!(directory.path().join("test.log").exists());
        assert!(directory.path().join("test.log.1").exists());
        assert!(
            !directory
                .path()
                .join(format!("test.log.{}", KEEP + 1))
                .exists(),
            "the oldest file should have been dropped"
        );
    }

    #[test]
    fn reopening_an_existing_log_appends_rather_than_truncating() {
        let directory = tempfile::tempdir().expect("temp dir");
        {
            let writer = RotatingWriter::new(directory.path(), "test.log").expect("writer");
            (&writer).write_all(b"before restart\n").expect("write");
            (&writer).flush().expect("flush");
        }
        {
            let writer = RotatingWriter::new(directory.path(), "test.log").expect("writer");
            (&writer).write_all(b"after restart\n").expect("write");
            (&writer).flush().expect("flush");
        }

        let contents = fs::read_to_string(directory.path().join("test.log")).expect("read");
        assert!(contents.contains("before restart"), "{contents}");
        assert!(contents.contains("after restart"), "{contents}");
    }

    #[test]
    fn the_level_can_be_changed_and_a_bad_level_is_refused() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (control, _writer) = init(directory.path(), "info", false).expect("init");

        assert_eq!(control.current(), "info");
        control.set("debug").expect("valid level");
        assert_eq!(control.current(), "debug");

        for bad in ["not a level!!", "verbose", "", "streamdeckd=info"] {
            assert!(control.set(bad).is_err(), "{bad} should be refused");
        }
        assert_eq!(
            control.current(),
            "debug",
            "a refused level changes nothing"
        );

        control.set("  WARN  ").expect("levels are normalized");
        assert_eq!(control.current(), "warn");
    }
}
