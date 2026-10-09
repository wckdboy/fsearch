import FSearchKit
import SwiftUI

@MainActor
@Observable
final class SearchModel {
    var query = ""
    var hits: [FSearchHit] = []
    var progress: FSearchProgress?
    var errorMessage: String?
    var importing = false
    var selectedPath: String?
    /// Bumps when the root set changes so the view refreshes even if the hit list does not.
    var generation = 0

    let bookmarks = BookmarkStore()
    private let watcher = DirectoryWatcher()
    private var engine: FSearch?
    private var searchTask: Task<Void, Never>?
    private var progressTask: Task<Void, Never>?
    private var refreshTask: Task<Void, Never>?
    private var bootstrapped = false

    var roots: [BookmarkStore.Saved] { bookmarks.saved }

    func bootstrap() async {
        guard !bootstrapped else { return }
        bootstrapped = true
        bookmarks.load()
        await restartEngine()
    }

    func addFolders(_ urls: [URL]) async {
        do {
            for url in urls {
                _ = try bookmarks.add(url)
            }
            await restartEngine()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func removeRoot(_ id: UUID) async {
        bookmarks.remove(id)
        selectedPath = nil
        await restartEngine()
    }

    func scheduleSearch() {
        searchTask?.cancel()
        let text = query
        searchTask = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            await runSearch(text)
        }
    }

    func refresh() async {
        guard let engine, progress?.ready == true else { return }
        do {
            try await engine.refresh()
            await runSearch(query)
        } catch {
            let failure = FSearchFailure(error)
            if failure.message != "indexing (first run scans the whole disk, ~20s)" && failure.message != "stopped" {
                errorMessage = failure.message
            }
        }
    }

    func scheduleRefresh() {
        refreshTask?.cancel()
        refreshTask = Task {
            try? await Task.sleep(for: .milliseconds(800))
            guard !Task.isCancelled else { return }
            await refresh()
        }
    }

    private func restartEngine() async {
        generation += 1
        searchTask?.cancel()
        progressTask?.cancel()
        if let engine {
            await engine.stop()
        }
        engine = nil
        hits = []
        progress = nil
        let urls = bookmarks.urls
        watcher.watch(urls) { [weak self] in
            Task { @MainActor in
                self?.scheduleRefresh()
            }
        }
        guard !urls.isEmpty else { return }
        let directory = BookmarkStore.indexDirectory
        do {
            let started = try await FSearch(roots: urls, indexDirectory: directory)
            engine = started
            attachProgress(started)
            scheduleSearch()
        } catch {
            errorMessage = FSearchFailure(error).message
        }
    }

    private func attachProgress(_ engine: FSearch) {
        progressTask = Task {
            var wasReady = false
            for await update in await engine.progress() {
                if Task.isCancelled { break }
                progress = update
                if update.ready && !wasReady {
                    wasReady = true
                    await runSearch(query)
                }
            }
        }
    }

    private func runSearch(_ text: String) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let engine, !trimmed.isEmpty else {
            hits = []
            return
        }
        do {
            let found = try await engine.search(query: trimmed, limit: 80)
            if query.trimmingCharacters(in: .whitespacesAndNewlines) == trimmed {
                hits = found
                if selectedPath == nil || !found.contains(where: { $0.path == selectedPath }) {
                    selectedPath = found.first?.path
                }
            }
        } catch {
            let failure = FSearchFailure(error)
            if failure.message.hasPrefix("indexing") || failure.message == "stopped" {
                return
            }
            errorMessage = failure.message
        }
    }
}
