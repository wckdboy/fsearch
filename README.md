# FSearch

Whole-disk file search for macOS. Finds any file by name in about a
millisecond, forgives typos, and searches inside files with an index. Use it
as a CLI (with a small daemon) or as a Rust crate.

This fork adds an iOS/iPadOS build, described below. The macOS CLI is the
upstream project by Noah Dunnagan,
[noahdunnagan/fsearch](https://github.com/noahdunnagan/fsearch), MIT.

```
cargo build --release && ./target/release/fsearch install   # -> ~/.local/bin/fsearch
fsearch fsearch main              # find files by name
fsearch 'ext:rs grep:apply_dir'   # search inside files
```

## Speed

M4 Max, 7.7M files and folders on disk.

| | |
|---|---|
| find a file by name, whole disk | p50 1.3 ms |
| search inside files | p50 9 ms |
| a new, renamed or deleted file shows up | ~0.1 s |
| first crawl of the disk | ~20 s, once |
| daemon memory | 30-135 MB |

## vs fff

Chromium (509k files), same Mac, same queries. Video:
[`demo/fsearch-vs-fff.mp4`](demo/fsearch-vs-fff.mp4), method:
[`demo/vs_fff.py`](demo/vs_fff.py).

| | fsearch | [fff](https://github.com/dmtrKovalenko/fff) |
|---|---|---|
| find a file by name | 1.1 ms | 13.8 ms |
| search inside files | 5.6 ms | 53 ms |
| typo still finds the file first | 98% | 88% |
| ready after launch | 50 ms | 2.5 s |
| memory | 50 MB (whole disk) | 358 MB (that folder) |

On the smaller Linux kernel (96k files), name search is a tie and fsearch
wins the rest. fff searches the contents of about 9% more files, because
fsearch skips some file types and `build/` and `vendor/` folders.

## Queries

```
fsearch 'readme in:~/Developer'          # inside a folder
fsearch 'type:image size:>5mb mtime:<7d'
fsearch 'ext:rs regex:fn\s+\w+_dir'      # regex inside files
fsearch 'sym:apply_dir'                  # where it's defined
```

Words are fuzzy, and 5+ letter words forgive one typo (`mian.rs` finds
`main.rs`). Also `'exact`, `^prefix`, `suffix$` and `!exclude`. Filters:
`ext:` `type:` `kind:` `in:` `size:` `mtime:` `re:` `path:` `grep:` `regex:`
`sym:` `limit:`. Content search is smart-case.

## Full Disk Access

Started from a terminal with Full Disk Access, it indexes everything. As a
login item (`fsearch install --login`), give `~/.local/bin/fsearch` its own
grant in System Settings > Privacy & Security, again after each rebuild.
Without access it skips the protected folders instead of popping a prompt.

## API

JSON lines over `~/Library/Application Support/FSearch/fsearch.sock`, or
`fsearch stdio`:

```json
{"q": "fsearch main", "limit": 20}
{"op": "grep", "pattern": "apply_dir", "in": "~/Developer"}
```

Or link the crate:

```rust
let engine = fsearch::Engine::start(fsearch::Options { dir: fsearch::default_dir(&home), home: home.clone(), skip: None })?;
let hits = engine.search(&fsearch::Query::parse("fsearch main", &home)?)?;
```

An app and the CLI share one index: the first process owns it and the
others follow along.

## How it works

- Crawls the disk once with `getattrlistbulk`, then stays current from
  FSEvents. A restart replays only what changed.
- Names live in one mmap'd file, laid out folder by folder so `in:` is a
  range. Each distinct name is scored once.
- Content search uses a trigram index of your text files. Matches are read
  fresh from disk, so they're never stale.

## iOS and iPadOS

iOS apps cannot see the whole disk, cannot run a login item, and cannot use
FSEvents. The same engine indexes a set of root folders in-process. The
macOS CLI is unchanged: `cargo build --release` still produces `fsearch`.

### What is different

| | macOS CLI | iOS / FSearchKit |
|---|---|---|
| Scope | `/`, with Full Disk Access | folders the user picked in Files |
| Updates | FSEvents | `refresh()` on foreground, a root re-open, and a button. A `DispatchSource` watches each root directory itself (not every nested folder) |
| Process | daemon, unix socket, LaunchAgent | in-process. No socket |
| Index files | `~/Library/Application Support/FSearch` | the app's Application Support (the demo uses `Application Support/FSearch/index`) |
| Crawl | `getattrlistbulk` | the same call when the kernel has it. `ENOSYS` falls back to a `read_dir` walker |

Picked folders are security-scoped bookmarks. The app has to keep
`startAccessingSecurityScopedResource()` active for as long as the engine
reads them, and store the bookmark data so the grant survives a relaunch.
Changing the root set deletes `index.bin` and rebuilds. Nested roots are
indexed once, by the parent.

Content search still skips generated trees (`build`, `target`, `Library`,
`node_modules`, and the rest of that list). The system can suspend the app
and pause a scan; coming forward refreshes. One engine per process: the
skip list and the extra content roots are process-wide, same as the macOS
daemon.

### Layout

- `Engine::start_roots` / `refresh` / `stop` / `progress` in the Rust crate.
  `Engine::start` (whole disk, FSEvents) stays `cfg`'d to macOS.
- `ffi/` is a UniFFI crate. Records, errors, and a `Send + Sync` object are
  generated into Swift, and a Swift actor hops the blocking calls onto
  detached tasks. A hand-written C ABI would have repeated that glue.
- `apple/build-xcframework.sh` builds `FSearchFFI.xcframework` for
  `aarch64-apple-ios`, the iOS simulator (`aarch64-apple-ios-sim` and
  `x86_64-apple-ios`, lipo'd), and macOS (`aarch64` and `x86_64`).
- `apple/FSearchKit` is a Swift package (iOS 17, macOS 14). `actor FSearch`
  has `search`, `grep`, `refresh`, `stop`, and `progress() -> AsyncStream`.
- `apple/FSearchDemo` is a SwiftUI app (iPhone and iPad). Open
  `apple/FSearchDemo/FSearchDemo.xcodeproj`. `project.yml` is the same
  project for XcodeGen.

### Build

On a Mac with Xcode and rustup:

```
./apple/build-xcframework.sh
```

That writes `apple/FSearchKit/FSearchFFI.xcframework` and refreshes the
generated Swift bindings. The package uses that local xcframework when it
is present. `FSEARCH_XCFRAMEWORK=/absolute/path` overrides it. Otherwise it
downloads the zip attached to an `ios-v*` GitHub release (the checksum in
`Package.swift` is filled in by the tag workflow; until the first release,
the local xcframework is the one that builds).

```
cd apple/FSearchKit && swift build          # macOS slice
open apple/FSearchDemo/FSearchDemo.xcodeproj
```

The demo target has signing turned off so CI can build it for the simulator.

Tag `ios-v*` (for example `ios-v0.1.0`) to upload
`FSearchFFI.xcframework.zip` and commit the new URL and checksum onto
`main`. The tag commit itself keeps the previous checksum.

### Using FSearchKit

In an app such as Omnie-dev, depend on the package and hold the actor:

```swift
let support = try FileManager.default.url(
    for: .applicationSupportDirectory,
    in: .userDomainMask,
    appropriateFor: nil,
    create: true
)
let engine = try await FSearch(
    roots: projectFolders, // security-scoped URLs you are still accessing
    indexDirectory: support.appendingPathComponent("FSearch/index")
)
for await update in await engine.progress() {
    // update.phase is scanning, ready, or refreshing; update.scanned counts names
}
let hits = try await engine.search(query: "actor FSearch ext:swift", limit: 40)
```

`search` returns path, name, score, kind, size, and mtime. `grep` uses a
`grep:` / `regex:` / `sym:` filter when the query has one, and otherwise
treats the string as a literal. Queries are the same language as the CLI
(`ext:`, `type:`, `in:`, `size:`, `mtime:`, …). Call `refresh()` when the
scene becomes active. `stop()` joins the index threads and drops the file
lock; releasing the actor does that too.

### Still to confirm on a device

The simulator build does not exercise a real Files provider, a persisted
security-scoped bookmark after the app is killed, Quick Look of a picked
file, or whether `getattrlistbulk` is present in the iOS libSystem you
link. If that symbol is missing, the portable walker is the fallback once
the call returns `ENOSYS`; a missing symbol at link time needs the Apple
path compiled out. Vnode watches do not see every nested change, so a
device pass should edit a file in a subfolder and confirm the foreground
refresh picks it up.
