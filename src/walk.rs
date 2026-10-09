//! Directory enumeration.
//!
//! On Apple platforms one `getattrlistbulk` returns hundreds of entries with
//! name, type, size, mtime and flags already attached, so there is no per-file
//! stat. Directories fan out over a rayon pool; mount points are not crossed,
//! firmlinks are (that is how /Users etc. on the data volume appear under /
//! exactly once). The same syscall exists on iOS. If it is missing (`ENOSYS`),
//! or this is not an Apple target, a portable `read_dir` walker produces the
//! same listing shape.

#[cfg(target_vendor = "apple")]
use rayon::Scope;
#[cfg(target_vendor = "apple")]
use std::cell::RefCell;
use std::ffi::CString;
#[cfg(target_vendor = "apple")]
use std::sync::Mutex;
#[cfg(target_vendor = "apple")]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const NONE: u32 = u32::MAX;

/// Folders never to open: set when the daemon runs without Full Disk
/// Access, where opening a consent-gated folder (Downloads, Desktop, ...)
/// pops a privacy prompt and blocks the call until someone answers it.
pub static SKIP: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// Rooted indexes (iOS, `Engine::start_roots`) were picked by the user, so
/// the Full Disk Access skip list must not hide them. One engine per process,
/// same as [`SKIP`].
static ALLOW_ALL: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_allow_all(on: bool) {
    ALLOW_ALL.store(on, Ordering::Relaxed);
}

pub fn blocked(path: &[u8]) -> bool {
    if ALLOW_ALL.load(Ordering::Relaxed) {
        return false;
    }
    SKIP.get().is_some_and(|v| v.iter().any(|s| path.starts_with(s) && (path.len() == s.len() || path[s.len()] == b'/')))
}

#[cfg(target_vendor = "apple")]
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
#[cfg(target_vendor = "apple")]
const DIR_MNTSTATUS_TRIGGER: u32 = 0x2;

pub const KIND_FILE: u8 = 0;
pub const KIND_DIR: u8 = 1;
pub const KIND_LINK: u8 = 2;
pub const KIND_OTHER: u8 = 3;

/// Entry flag bit: UF_HIDDEN set by the Finder.
pub const FLAG_HIDDEN: u8 = 1 << 2;
/// Directory that is a mount point we did not descend into.
pub const FLAG_MOUNT: u8 = 1 << 3;

#[derive(Clone, Copy)]
pub struct RawEnt {
    pub name_off: u32,
    pub name_len: u16,
    /// kind in the low 2 bits, FLAG_* above.
    pub kind: u8,
    pub size: u64,
    pub mtime: u32,
    /// Temp id of the listing for this directory, or NONE.
    pub child: u32,
}

pub struct Listing {
    pub id: u32,
    pub names: Vec<u8>,
    pub ents: Vec<RawEnt>,
}

static SCAN_COUNTING: AtomicBool = AtomicBool::new(false);
static SCAN_ENTRIES: AtomicU64 = AtomicU64::new(0);
static SCAN_DIRS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn reset_scan_count() {
    SCAN_ENTRIES.store(0, Ordering::Relaxed);
    SCAN_DIRS.store(0, Ordering::Relaxed);
    SCAN_COUNTING.store(true, Ordering::Relaxed);
}

pub(crate) fn finish_scan_count() {
    SCAN_COUNTING.store(false, Ordering::Relaxed);
}

pub(crate) fn scan_entries() -> u64 {
    SCAN_ENTRIES.load(Ordering::Relaxed)
}

pub(crate) fn scan_dirs() -> u64 {
    SCAN_DIRS.load(Ordering::Relaxed)
}

fn note_entry() {
    if SCAN_COUNTING.load(Ordering::Relaxed) {
        SCAN_ENTRIES.fetch_add(1, Ordering::Relaxed);
    }
}

fn note_dir() {
    if SCAN_COUNTING.load(Ordering::Relaxed) {
        SCAN_DIRS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Scan `root` recursively. Listing id 0 is `root` itself (its children).
pub fn scan(root: &[u8], threads: usize) -> Vec<Listing> {
    #[cfg(target_vendor = "apple")]
    if use_bulk() {
        return scan_bulk(root, threads);
    }
    let _ = threads;
    scan_portable(root)
}

/// List a single directory (no recursion). Subdirectories come back with
/// `child == NONE`. Used by the live updater.
pub fn list_one(path: &[u8]) -> Option<Listing> {
    if blocked(path) {
        return None;
    }
    #[cfg(target_vendor = "apple")]
    if use_bulk() {
        return list_one_bulk(path);
    }
    list_one_portable(path)
}

#[cfg(target_vendor = "apple")]
fn use_bulk() -> bool {
    static BULK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BULK.get_or_init(probe_bulk)
}

/// `false` only when the kernel does not implement the syscall. A failure to
/// open the working directory still leaves the fast path on: macOS and iOS
/// both have `getattrlistbulk`.
#[cfg(target_vendor = "apple")]
fn probe_bulk() -> bool {
    let fd = unsafe { libc::open(c".".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return true;
    }
    let mut al: libc::attrlist = unsafe { std::mem::zeroed() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr = libc::ATTR_CMN_NAME;
    let mut buf = [0u8; 4096];
    let n = unsafe { libc::getattrlistbulk(fd, &mut al as *mut _ as *mut libc::c_void, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
    let err = std::io::Error::last_os_error().raw_os_error();
    unsafe { libc::close(fd) };
    !(n < 0 && err == Some(libc::ENOSYS))
}

#[cfg(target_vendor = "apple")]
struct Ctx {
    next_id: AtomicU32,
    out: Vec<Mutex<Vec<Listing>>>,
}

#[cfg(target_vendor = "apple")]
thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 256 * 1024]);
}

#[cfg(target_vendor = "apple")]
fn scan_bulk(root: &[u8], threads: usize) -> Vec<Listing> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).start_handler(|_| crate::no_materialize()).build().unwrap();
    let ctx = Ctx { next_id: AtomicU32::new(1), out: (0..threads + 1).map(|_| Mutex::new(Vec::new())).collect() };
    raise_fd_limit();
    let fd = if blocked(root) { -1 } else { CString::new(root).map_or(-1, |c| unsafe { libc::open(c.as_ptr(), OPEN_DIR) }) };
    // Paths are only tracked when there is something to skip.
    let path = (!ALLOW_ALL.load(Ordering::Relaxed) && SKIP.get().is_some_and(|v| !v.is_empty())).then(|| root.to_vec());
    pool.scope(|s| finish_dir(s, fd, path, 0, &ctx));
    ctx.out.into_iter().flat_map(|m| m.into_inner().unwrap()).collect()
}

#[cfg(target_vendor = "apple")]
fn raise_fd_limit() {
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) };
    r.rlim_cur = r.rlim_max.min(65536);
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &r) };
}

#[cfg(target_vendor = "apple")]
fn list_one_bulk(path: &[u8]) -> Option<Listing> {
    let mut l = Listing { id: 0, names: Vec::new(), ents: Vec::new() };
    list_into(path, &mut l).then_some(l)
}

#[cfg(target_vendor = "apple")]
const OPEN_DIR: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Directory fd shared by the tasks that still need to openat() a child.
#[cfg(target_vendor = "apple")]
struct Fd(i32);
#[cfg(target_vendor = "apple")]
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

// Children are opened with openat() relative to the parent's fd, so paths
// never get rebuilt and PATH_MAX never bites. Measured on this Mac: open() +
// close() is ~19us per directory (two Endpoint Security clients tax every
// open), getattrlistbulk ~14us; past ~8 threads the kernel side stops scaling.
#[cfg(target_vendor = "apple")]
fn finish_dir<'s>(s: &Scope<'s>, fd: i32, path: Option<Vec<u8>>, id: u32, ctx: &'s Ctx) {
    let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
    if fd < 0 {
        note_dir();
        push(l, ctx);
        return;
    }
    list_fd(fd, &mut l);
    note_dir();
    let me = std::sync::Arc::new(Fd(fd));
    let mut kids = Vec::new();
    for e in l.ents.iter_mut() {
        if e.kind & 3 == KIND_DIR && e.kind & FLAG_MOUNT == 0 {
            let name = &l.names[e.name_off as usize..e.name_off as usize + e.name_len as usize];
            let child_path = path.as_ref().map(|p| crate::live::join(p, name));
            if child_path.as_deref().is_some_and(blocked) {
                continue;
            }
            e.child = ctx.next_id.fetch_add(1, Ordering::Relaxed);
            kids.push((CString::new(name).unwrap_or_default(), e.child, child_path));
        }
    }
    push(l, ctx);
    for (name, cid, child_path) in kids {
        let parent = me.clone();
        s.spawn(move |s| {
            let fd = unsafe { libc::openat(parent.0, name.as_ptr(), OPEN_DIR) };
            drop(parent);
            finish_dir(s, fd, child_path, cid, ctx);
        });
    }
}

#[cfg(target_vendor = "apple")]
fn push(l: Listing, ctx: &Ctx) {
    let slot = rayon::current_thread_index().unwrap_or(ctx.out.len() - 1);
    ctx.out[slot].lock().unwrap().push(l);
}

#[cfg(target_vendor = "apple")]
fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}

#[cfg(target_vendor = "apple")]
fn rd64(b: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(b[at..at + 8].try_into().unwrap())
}

/// Returns false if the directory could not be opened.
#[cfg(target_vendor = "apple")]
fn list_into(path: &[u8], l: &mut Listing) -> bool {
    let Ok(cpath) = CString::new(path) else { return false };
    let fd = unsafe { libc::open(cpath.as_ptr(), OPEN_DIR) };
    if fd < 0 {
        return false;
    }
    list_fd(fd, l);
    unsafe { libc::close(fd) };
    true
}

#[cfg(target_vendor = "apple")]
fn list_fd(fd: i32, l: &mut Listing) {
    let mut al: libc::attrlist = unsafe { std::mem::zeroed() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr =
        libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME | ATTR_CMN_ERROR | libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_MODTIME | libc::ATTR_CMN_FLAGS;
    al.dirattr = libc::ATTR_DIR_MOUNTSTATUS;
    al.fileattr = libc::ATTR_FILE_DATALENGTH;
    BUF.with_borrow_mut(|buf| {
        loop {
            let n = unsafe { libc::getattrlistbulk(fd, &mut al as *mut _ as *mut libc::c_void, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
            if n <= 0 {
                break;
            }
            let mut p = 0usize;
            for _ in 0..n {
                let len = rd32(buf, p) as usize;
                parse_entry(&buf[p..p + len], l);
                p += len;
            }
        }
    });
}

#[cfg(target_vendor = "apple")]
fn parse_entry(b: &[u8], l: &mut Listing) {
    let common = rd32(b, 4);
    let dirattr = rd32(b, 12);
    let fileattr = rd32(b, 16);
    let mut f = 24;
    if common & ATTR_CMN_ERROR != 0 {
        f += 4;
    }
    if common & libc::ATTR_CMN_NAME == 0 {
        return;
    }
    let off = rd32(b, f) as i32 as isize;
    let nlen = rd32(b, f + 4) as usize;
    let start = (f as isize + off) as usize;
    let name = &b[start..start + nlen.saturating_sub(1)];
    f += 8;
    let mut kind = KIND_OTHER;
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        kind = match rd32(b, f) {
            1 => KIND_FILE,
            2 => KIND_DIR,
            5 => KIND_LINK,
            _ => KIND_OTHER,
        };
        f += 4;
    }
    let mut mtime = 0u32;
    if common & libc::ATTR_CMN_MODTIME != 0 {
        mtime = (rd64(b, f) as i64).clamp(0, u32::MAX as i64) as u32;
        f += 16;
    }
    if common & libc::ATTR_CMN_FLAGS != 0 {
        if rd32(b, f) & libc::UF_HIDDEN != 0 {
            kind |= FLAG_HIDDEN;
        }
        f += 4;
    }
    if dirattr & libc::ATTR_DIR_MOUNTSTATUS != 0 {
        if rd32(b, f) & (libc::DIR_MNTSTATUS_MNTPOINT | DIR_MNTSTATUS_TRIGGER) != 0 {
            kind |= FLAG_MOUNT;
        }
        f += 4;
    }
    let mut size = 0u64;
    if fileattr & libc::ATTR_FILE_DATALENGTH != 0 {
        size = rd64(b, f);
    }
    if name.is_empty() || name.len() > u16::MAX as usize {
        return;
    }
    l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size, mtime, child: NONE });
    l.names.extend_from_slice(name);
    note_entry();
}

/// `read_dir` walker used where `getattrlistbulk` is absent. Symlinks are not
/// followed and a directory whose device differs from `root` is a mount.
pub fn scan_portable(root: &[u8]) -> Vec<Listing> {
    let mut out = Vec::new();
    let mut next = 1u32;
    if blocked(root) {
        note_dir();
        out.push(Listing { id: 0, names: Vec::new(), ents: Vec::new() });
        return out;
    }
    let dev = device(root);
    walk_dir(root, 0, &mut next, &mut out, dev);
    out
}

fn list_one_portable(path: &[u8]) -> Option<Listing> {
    let mut l = Listing { id: 0, names: Vec::new(), ents: Vec::new() };
    fill_dir(path, &mut l, None)?;
    Some(l)
}

fn walk_dir(path: &[u8], id: u32, next: &mut u32, out: &mut Vec<Listing>, root_dev: Option<u64>) {
    note_dir();
    let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
    let mut kids = Vec::new();
    if fill_dir(path, &mut l, Some(&mut kids)).is_none() {
        out.push(l);
        return;
    }
    for e in l.ents.iter_mut() {
        if e.kind & 3 != KIND_DIR {
            continue;
        }
        let name = &l.names[e.name_off as usize..][..e.name_len as usize];
        let child_path = crate::live::join(path, name);
        let dev = device(&child_path);
        if root_dev.is_some() && dev.is_some() && dev != root_dev {
            e.kind |= FLAG_MOUNT;
            continue;
        }
        if blocked(&child_path) {
            continue;
        }
        e.child = *next;
        *next += 1;
        kids.push((child_path, e.child));
    }
    out.push(l);
    for (p, cid) in kids {
        walk_dir(&p, cid, next, out, root_dev);
    }
}

/// `None` when `path` cannot be opened. `kids` is unused; descent is decided
/// by the caller from the listing. Returns `Some(())` on a successful open,
/// including an empty directory.
fn fill_dir(path: &[u8], l: &mut Listing, _kids: Option<&mut Vec<(Vec<u8>, u32)>>) -> Option<()> {
    use std::os::unix::ffi::OsStrExt;
    let rd = std::fs::read_dir(std::path::Path::new(std::ffi::OsStr::from_bytes(path))).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        let nb = name.as_bytes();
        if nb.is_empty() || nb.len() > u16::MAX as usize || nb.contains(&0) {
            continue;
        }
        let child_path = crate::live::join(path, nb);
        let Some(meta) = lstat_meta(&child_path) else { continue };
        l.ents.push(RawEnt {
            name_off: l.names.len() as u32,
            name_len: nb.len() as u16,
            kind: meta.kind,
            size: meta.size,
            mtime: meta.mtime,
            child: NONE,
        });
        l.names.extend_from_slice(nb);
        note_entry();
    }
    Some(())
}

struct Meta {
    kind: u8,
    size: u64,
    mtime: u32,
    dev: u64,
}

fn lstat_meta(path: &[u8]) -> Option<Meta> {
    let c = CString::new(path).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(unused_mut)]
    let mut kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => KIND_FILE,
        libc::S_IFDIR => KIND_DIR,
        libc::S_IFLNK => KIND_LINK,
        _ => KIND_OTHER,
    };
    #[cfg(target_vendor = "apple")]
    if st.st_flags & libc::UF_HIDDEN != 0 {
        kind |= FLAG_HIDDEN;
    }
    let size = if kind & 3 == KIND_FILE { st.st_size.max(0) as u64 } else { 0 };
    let mtime = st.st_mtime.clamp(0, u32::MAX as i64) as u32;
    Some(Meta { kind, size, mtime, dev: st.st_dev as u64 })
}

fn device(path: &[u8]) -> Option<u64> {
    lstat_meta(path).map(|m| m.dev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::for_each_path;
    use std::os::unix::ffi::OsStrExt;

    fn scratch(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fsearch-walk-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn paths(ls: &[Listing], root: &std::path::Path) -> Vec<(String, u8)> {
        let mut out = Vec::new();
        for_each_path(ls, root.as_os_str().as_bytes(), |p, r| {
            out.push((String::from_utf8_lossy(&p).into_owned(), r.kind & 3));
        });
        out.sort();
        out
    }

    #[test]
    fn portable_lists_files_dirs_and_does_not_follow_symlinks() {
        let dir = scratch("port");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("hello world.txt"), b"hi").unwrap();
        std::fs::write(dir.join("sub").join("inner.rs"), b"fn").unwrap();
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("link-to-sub")).unwrap();
        let ls = scan_portable(dir.as_os_str().as_bytes());
        let got = paths(&ls, &dir);
        let hello = dir.join("hello world.txt");
        let inner = dir.join("sub").join("inner.rs");
        assert!(got.iter().any(|(p, k)| p == hello.to_str().unwrap() && *k == KIND_FILE));
        assert!(got.iter().any(|(p, k)| p == inner.to_str().unwrap() && *k == KIND_FILE));
        assert!(got.iter().any(|(p, k)| p.ends_with("link-to-sub") && *k == KIND_LINK));
        assert!(!got.iter().any(|(p, _)| p.contains("link-to-sub/")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_list_one_does_not_recurse() {
        let dir = scratch("one");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        let l = list_one(dir.as_os_str().as_bytes()).unwrap();
        let names: Vec<_> =
            l.ents.iter().map(|e| String::from_utf8_lossy(&l.names[e.name_off as usize..][..e.name_len as usize]).into_owned()).collect();
        assert!(names.iter().any(|n| n == "a.txt"));
        assert!(names.iter().any(|n| n == "sub"));
        assert!(l.ents.iter().all(|e| e.child == NONE));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
