//! Index a set of folders instead of `/`.
//!
//! iOS (and any embedded caller) cannot see the whole disk. The user picks
//! folders; each one is scanned on its own and stitched into the same `/`-rooted
//! name index the macOS crawler produces, so `in:` and path lookup keep working.
//! Ancestors of a root are recorded as directories so the path resolves, but
//! their siblings are not listed: those folders were not granted.

use crate::live::{self, OEnt};
use crate::walk::{self, FLAG_MOUNT, KIND_DIR, KIND_FILE, Listing, NONE, RawEnt};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, RwLock};

static HAS_EXTRA: AtomicBool = AtomicBool::new(false);
static CONTENT_ROOTS: LazyLock<RwLock<Arc<Vec<Vec<u8>>>>> = LazyLock::new(|| RwLock::new(Arc::new(Vec::new())));

/// Folders that count as indexed for content search, in addition to `$HOME`.
/// Empty on the whole-disk engine, so macOS scope checks stay as they were.
pub(crate) fn content_roots() -> Arc<Vec<Vec<u8>>> {
    CONTENT_ROOTS.read().unwrap().clone()
}

pub(crate) fn set_content_roots(roots: Vec<Vec<u8>>) {
    HAS_EXTRA.store(!roots.is_empty(), Ordering::Relaxed);
    *CONTENT_ROOTS.write().unwrap() = Arc::new(roots);
}

pub(crate) fn has_extra_roots() -> bool {
    HAS_EXTRA.load(Ordering::Relaxed)
}

/// `path` is `root` or a descendant. The returned slice is the relative
/// remainder without a leading slash (empty when `path == root`).
pub(crate) fn relative_to<'a>(path: &'a [u8], root: &[u8]) -> Option<&'a [u8]> {
    if root == b"/" {
        return None;
    }
    if path == root {
        return Some(b"");
    }
    let rest = path.strip_prefix(root)?;
    if rest.first() == Some(&b'/') { Some(&rest[1..]) } else { None }
}

/// Scan `roots` into one listing tree whose id 0 is `/`.
pub(crate) fn scan_roots(roots: &[Vec<u8>], threads: usize) -> Vec<Listing> {
    let roots = collapse(roots);
    if roots.iter().any(|r| r.as_slice() == b"/") {
        return walk::scan(b"/", threads);
    }
    if roots.is_empty() {
        return vec![Listing { id: 0, names: Vec::new(), ents: Vec::new() }];
    }
    let mut next = 1u32;
    let mut extras = Vec::new();
    let mut top = Synth::default();
    for root in roots {
        let meta = live::lstat(&root).unwrap_or(OEnt::new(&root, KIND_DIR, 0, 0));
        let is_dir = meta.kind & 3 == KIND_DIR && meta.kind & FLAG_MOUNT == 0;
        let contents = if is_dir {
            let scanned = walk::scan(&root, threads);
            Some(attach_scan(scanned, &mut next, &mut extras))
        } else {
            None
        };
        insert(&mut top, &root, Leaf { kind: meta.kind, size: meta.size, mtime: meta.mtime, contents });
    }
    let mut out = Vec::new();
    emit(&top, 0, &mut next, &mut out);
    out.extend(extras);
    out
}

/// Drop a root that lives inside another, and `/` when anything else is present
/// is handled by the caller. Parents sort first, so one pass is enough.
fn collapse(roots: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = roots.iter().map(|r| live::normalize(r)).filter(|r| !r.is_empty()).collect();
    v.sort();
    v.dedup();
    let mut out: Vec<Vec<u8>> = Vec::new();
    for r in v {
        if out.iter().any(|p| r.starts_with(p) && r.get(p.len()) == Some(&b'/')) {
            continue;
        }
        out.push(r);
    }
    out
}

fn attach_scan(scanned: Vec<Listing>, next: &mut u32, extras: &mut Vec<Listing>) -> u32 {
    if scanned.is_empty() {
        let id = *next;
        *next += 1;
        extras.push(Listing { id, names: Vec::new(), ents: Vec::new() });
        return id;
    }
    let mut map = std::collections::HashMap::with_capacity(scanned.len());
    for l in &scanned {
        map.insert(l.id, *next);
        *next += 1;
    }
    let root_id = map[&0];
    for l in scanned {
        let mut nl = Listing { id: map[&l.id], names: l.names, ents: l.ents };
        for e in &mut nl.ents {
            if e.child != NONE {
                e.child = map[&e.child];
            }
        }
        extras.push(nl);
    }
    root_id
}

struct Leaf {
    kind: u8,
    size: u64,
    mtime: u32,
    contents: Option<u32>,
}

#[derive(Default)]
struct Synth {
    kind: u8,
    size: u64,
    mtime: u32,
    kids: BTreeMap<Vec<u8>, Child>,
}

enum Child {
    Synth(Synth),
    Leaf(Leaf),
}

fn insert(top: &mut Synth, path: &[u8], leaf: Leaf) {
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    fn rec(node: &mut Synth, parts: &[&[u8]], prefix: &[u8], leaf: Leaf) {
        let name = parts[0];
        let so_far = join_path(prefix, name);
        if parts.len() == 1 {
            node.kids.insert(name.to_vec(), Child::Leaf(leaf));
            return;
        }
        let child = node.kids.entry(name.to_vec()).or_insert_with(|| {
            let meta = live::lstat(&so_far).unwrap_or(OEnt::new(&so_far, KIND_DIR, 0, 0));
            let kind = if meta.kind & 3 == KIND_FILE { KIND_DIR } else { meta.kind };
            Child::Synth(Synth { kind, size: 0, mtime: meta.mtime, kids: BTreeMap::new() })
        });
        match child {
            Child::Synth(sub) => rec(sub, &parts[1..], &so_far, leaf),
            // A previous root was an ancestor; `collapse` drops this case.
            Child::Leaf(_) => {}
        }
    }
    rec(top, &parts, b"", leaf);
}

fn join_path(prefix: &[u8], name: &[u8]) -> Vec<u8> {
    if prefix.is_empty() || prefix == b"/" {
        let mut p = Vec::with_capacity(1 + name.len());
        p.push(b'/');
        p.extend_from_slice(name);
        p
    } else {
        let mut p = Vec::with_capacity(prefix.len() + 1 + name.len());
        p.extend_from_slice(prefix);
        p.push(b'/');
        p.extend_from_slice(name);
        p
    }
}

fn emit(node: &Synth, id: u32, next: &mut u32, out: &mut Vec<Listing>) {
    let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
    for (name, child) in &node.kids {
        match child {
            Child::Leaf(leaf) => push(&mut l, name, leaf.kind, leaf.size, leaf.mtime, leaf.contents.unwrap_or(NONE)),
            Child::Synth(sub) => {
                let cid = *next;
                *next += 1;
                emit(sub, cid, next, out);
                let kind = if sub.kind == 0 { KIND_DIR } else { sub.kind };
                push(&mut l, name, kind, sub.size, sub.mtime, cid);
            }
        }
    }
    out.push(l);
}

fn push(l: &mut Listing, name: &[u8], kind: u8, size: u64, mtime: u32, child: u32) {
    if name.is_empty() || name.len() > u16::MAX as usize {
        return;
    }
    l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size, mtime, child });
    l.names.extend_from_slice(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn scratch(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fsearch-roots-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn b(p: &Path) -> Vec<u8> {
        p.as_os_str().as_bytes().to_vec()
    }

    #[test]
    fn two_roots_keep_their_paths() {
        let dir = scratch("two");
        let a = dir.join("a");
        let bdir = dir.join("b");
        std::fs::create_dir_all(a.join("nested")).unwrap();
        std::fs::create_dir_all(&bdir).unwrap();
        std::fs::write(a.join("nested").join("one.txt"), b"1").unwrap();
        std::fs::write(bdir.join("two.txt"), b"2").unwrap();
        let ls = scan_roots(&[b(&a), b(&bdir)], 2);
        let idx = Index::build(ls, 0, 0, &b(&dir));
        assert!(idx.lookup(&b(&a.join("nested").join("one.txt"))).is_some(), "nested file");
        assert!(idx.lookup(&b(&bdir.join("two.txt"))).is_some(), "sibling root");
        assert!(idx.lookup(&b(&a)).is_some(), "root dir itself");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_root_is_not_scanned_twice() {
        let dir = scratch("nest");
        let parent = dir.join("parent");
        std::fs::create_dir_all(parent.join("child")).unwrap();
        std::fs::write(parent.join("child").join("f.txt"), b"f").unwrap();
        let only = scan_roots(&[b(&parent)], 2);
        let both = scan_roots(&[b(&parent), b(&parent.join("child"))], 2);
        let a = Index::build(only, 0, 0, &b(&dir));
        let bld = Index::build(both, 0, 0, &b(&dir));
        assert_eq!(a.n, bld.n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_to_rejects_prefix_cousins() {
        assert_eq!(relative_to(b"/tmp/ab/c", b"/tmp/a"), None);
        assert_eq!(relative_to(b"/tmp/a/c", b"/tmp/a"), Some(b"c".as_slice()));
        assert_eq!(relative_to(b"/tmp/a", b"/tmp/a"), Some(b"".as_slice()));
    }

    #[test]
    fn file_root_is_a_single_entry() {
        let dir = scratch("file");
        let f = dir.join("solo.txt");
        std::fs::write(&f, b"solo").unwrap();
        let ls = scan_roots(&[b(&f)], 1);
        let idx = Index::build(ls, 0, 0, &b(&dir));
        let e = idx.lookup(&b(&f)).expect("file");
        assert_eq!(idx.kind()[e as usize] & 3, KIND_FILE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
