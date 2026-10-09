import FSearchFFIBindings
import Foundation

public enum FSearchKind: String, Sendable, Hashable {
    case file
    case dir
    case link
    case other
}

public struct FSearchHit: Sendable, Hashable, Identifiable {
    public var id: String { path }
    public var path: String
    public var name: String
    public var score: Int
    public var kind: FSearchKind
    public var size: UInt64
    /// Modification time, unix seconds.
    public var mtime: UInt32

    public var modified: Date {
        Date(timeIntervalSince1970: TimeInterval(mtime))
    }
}

public struct FSearchGrepMatch: Sendable, Hashable, Identifiable {
    public var id: String { "\(path):\(line):\(text)" }
    public var path: String
    public var line: Int
    public var text: String
}

public struct FSearchProgress: Sendable, Equatable {
    public var phase: Phase
    public var ready: Bool
    public var entries: UInt64
    public var scanned: UInt64
    public var dirsScanned: UInt64

    public enum Phase: String, Sendable {
        case idle
        case scanning
        case ready
        case refreshing
        case stopped
        case unknown
    }

    public var isWorking: Bool {
        switch phase {
        case .scanning, .refreshing:
            true
        case .idle:
            !ready
        case .ready, .stopped, .unknown:
            false
        }
    }
}

public struct FSearchFailure: Error, Sendable, Equatable {
    public var message: String

    public init(_ error: Error) {
        message = failureMessage(error)
    }
}

extension FSearchFailure: LocalizedError {
    public var errorDescription: String? { message }
}

/// In-process name and content search over the folders the user picked.
///
/// Blocking FFI calls run on a detached task so the actor stays free to
/// publish progress. `progress()` polls the engine; there is no FSEvents
/// stream on iOS.
public actor FSearch {
    private let box: EngineBox

    public init(roots: [URL], indexDirectory: URL) async throws {
        let paths = roots.map(\.path)
        let directory = indexDirectory.path
        let engine = try await Task.detached(priority: .userInitiated) {
            try FSearchEngine.start(roots: paths, indexDir: directory)
        }.value
        box = EngineBox(engine)
    }

    public func search(query: String, limit: Int = 50) async throws -> [FSearchHit] {
        let engine = box.engine
        let cap = UInt32(clamping: limit)
        let hits = try await Task.detached(priority: .userInitiated) {
            try engine.search(query: query, limit: cap)
        }.value
        return hits.map(FSearchHit.init)
    }

    public func grep(query: String, limit: Int = 50) async throws -> [FSearchGrepMatch] {
        let engine = box.engine
        let cap = UInt32(clamping: limit)
        let matches = try await Task.detached(priority: .userInitiated) {
            try engine.grep(query: query, limit: cap)
        }.value
        return matches.map { FSearchGrepMatch(path: $0.path, line: Int($0.line), text: $0.text) }
    }

    public func refresh() async throws {
        let engine = box.engine
        try await Task.detached(priority: .utility) {
            try engine.refresh()
        }.value
    }

    public func stop() async {
        let engine = box.engine
        await Task.detached(priority: .utility) {
            engine.stop()
        }.value
    }

    public func progress(every interval: Duration = .milliseconds(200)) -> AsyncStream<FSearchProgress> {
        let engine = box.engine
        return AsyncStream { continuation in
            let task = Task {
                while !Task.isCancelled {
                    continuation.yield(FSearchProgress(engine.status()))
                    try? await Task.sleep(for: interval)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }
}

private extension FSearchHit {
    init(_ hit: SearchHit) {
        path = hit.path
        name = hit.name
        score = Int(hit.score)
        kind = FSearchKind(rawValue: hit.kind) ?? .other
        size = hit.size
        mtime = hit.mtime
    }
}

private extension FSearchProgress {
    init(_ status: IndexStatus) {
        phase = Phase(rawValue: status.phase) ?? .unknown
        ready = status.ready
        entries = status.entries
        scanned = status.scanned
        dirsScanned = status.dirsScanned
    }
}

private func failureMessage(_ error: Error) -> String {
    guard let searchError = error as? FSearchError else {
        return error.localizedDescription
    }
    switch searchError {
    case .Engine(let message):
        return message
    }
}

/// Calls `stop()` when the actor is released, including the join of the
/// index thread. `FSearchEngine.stop` is safe to call more than once.
private final class EngineBox: @unchecked Sendable {
    let engine: FSearchEngine
    init(_ engine: FSearchEngine) {
        self.engine = engine
    }
    deinit {
        engine.stop()
    }
}
