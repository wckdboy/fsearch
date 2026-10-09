//! In-process bindings for the iOS/macOS Swift package.
//!
//! UniFFI (rather than a hand-written C ABI) because the surface is records,
//! errors, and a thread-safe object: it generates the Swift types, the
//! module map, and the retain/release glue, and the object is `Send + Sync`
//! so a Swift actor can call it from detached tasks without pinning a thread.
//! The methods themselves are synchronous; Swift moves them off the main actor.

use std::path::PathBuf;
use std::sync::Mutex;

use fsearch::{Engine, Grep, GrepMode, Query};

uniffi::setup_scaffolding!();

#[derive(uniffi::Record)]
pub struct SearchHit {
    pub path: String,
    pub name: String,
    pub score: i32,
    pub kind: String,
    pub size: u64,
    pub mtime: u32,
}

#[derive(uniffi::Record)]
pub struct GrepMatch {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(uniffi::Record)]
pub struct IndexStatus {
    /// `idle`, `scanning`, `ready`, `refreshing`, or `stopped`.
    pub phase: String,
    pub ready: bool,
    pub entries: u64,
    pub scanned: u64,
    pub dirs_scanned: u64,
}

#[derive(Debug, uniffi::Error)]
pub enum FSearchError {
    Engine { message: String },
}

impl std::fmt::Display for FSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FSearchError::Engine { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for FSearchError {}

fn err(message: impl Into<String>) -> FSearchError {
    FSearchError::Engine { message: message.into() }
}

#[derive(uniffi::Object)]
pub struct FSearchEngine {
    inner: Mutex<Option<Engine>>,
}

#[uniffi::export]
impl FSearchEngine {
    /// Index `roots` (absolute paths the app can read) into `index_dir`.
    /// Returns immediately; poll [`Self::status`] until `ready`.
    #[uniffi::constructor]
    pub fn start(roots: Vec<String>, index_dir: String) -> Result<std::sync::Arc<Self>, FSearchError> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let paths: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        let engine = Engine::start_roots(fsearch::Options { dir: PathBuf::from(index_dir), home, skip: None }, paths).map_err(err)?;
        Ok(std::sync::Arc::new(Self { inner: Mutex::new(Some(engine)) }))
    }

    pub fn search(&self, query: String, limit: u32) -> Result<Vec<SearchHit>, FSearchError> {
        let engine = self.engine()?;
        let mut q = Query::parse(&query, engine.home()).map_err(err)?;
        if limit > 0 {
            q.limit = limit as usize;
        }
        let found = engine.search(&q).map_err(err)?;
        Ok(found
            .into_iter()
            .map(|h| {
                let name = h.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                SearchHit {
                    path: h.path.to_string_lossy().into_owned(),
                    name,
                    score: h.score,
                    kind: kind_name(h.kind).into(),
                    size: h.size,
                    mtime: h.mtime,
                }
            })
            .collect())
    }

    /// Content search. A `grep:` / `regex:` / `sym:` filter in `query` sets
    /// the pattern and mode; otherwise the whole string is a literal pattern.
    /// Other filters (`ext:`, `in:`, …) still narrow which files are read.
    pub fn grep(&self, query: String, limit: u32) -> Result<Vec<GrepMatch>, FSearchError> {
        let engine = self.engine()?;
        let mut q = Query::parse(&query, engine.home()).map_err(err)?;
        if limit > 0 {
            q.limit = limit as usize;
        }
        let (pattern, mode) = match q.grep.clone() {
            Some(p) => (p, q.grep_mode),
            None => (query, GrepMode::Literal),
        };
        let g = Grep::new(&pattern, mode).map_err(err)?;
        let (result, _) = engine.grep(&q, &g).map_err(err)?;
        let mut out = Vec::new();
        for file in result.files {
            let path = String::from_utf8_lossy(&file.path).into_owned();
            for (line, text) in file.lines {
                out.push(GrepMatch { path: path.clone(), line: line as u32, text });
            }
        }
        Ok(out)
    }

    pub fn refresh(&self) -> Result<(), FSearchError> {
        self.engine()?.refresh().map_err(err)
    }

    pub fn stop(&self) {
        if let Some(engine) = self.inner.lock().unwrap().take() {
            engine.stop();
        }
    }

    pub fn status(&self) -> IndexStatus {
        let Ok(engine) = self.engine() else {
            return IndexStatus { phase: "stopped".into(), ready: false, entries: 0, scanned: 0, dirs_scanned: 0 };
        };
        let p = engine.progress();
        IndexStatus { phase: p.phase, ready: p.ready, entries: p.entries, scanned: p.scanned, dirs_scanned: p.dirs_scanned }
    }
}

impl FSearchEngine {
    fn engine(&self) -> Result<Engine, FSearchError> {
        self.inner.lock().unwrap().clone().ok_or_else(|| err("stopped"))
    }
}

fn kind_name(k: u8) -> &'static str {
    match k & 3 {
        fsearch::walk::KIND_FILE => "file",
        fsearch::walk::KIND_DIR => "dir",
        fsearch::walk::KIND_LINK => "link",
        _ => "other",
    }
}
