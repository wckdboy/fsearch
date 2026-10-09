import Darwin
import Foundation

/// Best-effort vnode watch on each root directory.
///
/// Dispatch sources fire for writes in that directory, not reliably for every
/// nested change. The demo also refreshes when the scene becomes active.
final class DirectoryWatcher: @unchecked Sendable {
    private let queue = DispatchQueue(label: "dev.omnie.fsearch.watch")
    private var sources: [DispatchSourceFileSystemObject] = []

    func watch(_ urls: [URL], onChange: @escaping @Sendable () -> Void) {
        queue.async {
            self.stopLocked()
            for url in urls {
                let fd = open(url.path, O_EVTONLY)
                if fd < 0 { continue }
                let source = DispatchSource.makeFileSystemObjectSource(
                    fileDescriptor: fd,
                    eventMask: [.write, .rename, .delete, .extend, .attrib],
                    queue: self.queue
                )
                source.setEventHandler(handler: onChange)
                source.setCancelHandler { close(fd) }
                source.resume()
                self.sources.append(source)
            }
        }
    }

    func stop() {
        queue.async { self.stopLocked() }
    }

    private func stopLocked() {
        for source in sources {
            source.cancel()
        }
        sources.removeAll()
    }
}
