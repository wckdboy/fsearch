import Foundation

/// Security-scoped bookmarks for folders the user picked in Files.
/// Access is held for the life of the store so the indexer can read them.
@MainActor
final class BookmarkStore {
    struct Saved: Codable, Identifiable, Equatable {
        var id: UUID
        var name: String
        var bookmark: Data
    }

    private(set) var saved: [Saved] = []
    private var open: [UUID: URL] = [:]
    private let fileURL: URL

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("FSearch", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        fileURL = base.appendingPathComponent("bookmarks.json")
    }

    static var indexDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("FSearch", isDirectory: true)
        try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
        return base.appendingPathComponent("index", isDirectory: true)
    }

    var urls: [URL] {
        saved.compactMap { open[$0.id] }
    }

    func load() {
        guard let data = try? Data(contentsOf: fileURL),
              let decoded = try? JSONDecoder().decode([Saved].self, from: data) else {
            return
        }
        saved = decoded
        for item in saved {
            _ = resolve(item)
        }
    }

    @discardableResult
    func add(_ url: URL) throws -> URL {
        let data = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
        let name = url.lastPathComponent
        let item = Saved(id: UUID(), name: name, bookmark: data)
        let resolved = try resolveOrThrow(item)
        saved.append(item)
        persist()
        return resolved
    }

    func remove(_ id: UUID) {
        if let url = open.removeValue(forKey: id) {
            url.stopAccessingSecurityScopedResource()
        }
        saved.removeAll { $0.id == id }
        persist()
    }

    private func resolve(_ item: Saved) -> URL? {
        try? resolveOrThrow(item)
    }

    private func resolveOrThrow(_ item: Saved) throws -> URL {
        var stale = false
        let url = try URL(resolvingBookmarkData: item.bookmark, options: [], relativeTo: nil, bookmarkDataIsStale: &stale)
        _ = url.startAccessingSecurityScopedResource()
        open[item.id] = url
        if stale {
            if let fresh = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil),
               let index = saved.firstIndex(where: { $0.id == item.id }) {
                saved[index].bookmark = fresh
                persist()
            }
        }
        return url
    }

    private func persist() {
        guard let data = try? JSONEncoder().encode(saved) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }
}
