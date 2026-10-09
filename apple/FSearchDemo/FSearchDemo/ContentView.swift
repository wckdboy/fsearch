import FSearchKit
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @Bindable var model: SearchModel

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationTitle("FSearch")
                .toolbar { toolbar }
        } detail: {
            detail
        }
        .fileImporter(isPresented: $model.importing, allowedContentTypes: [.folder], allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls):
                Task { await model.addFolders(urls) }
            case .failure(let error):
                model.errorMessage = error.localizedDescription
            }
        }
        .alert("Search", isPresented: errorShown) {
            Button("OK", role: .cancel) { model.errorMessage = nil }
        } message: {
            Text(model.errorMessage ?? "")
        }
    }

    private var errorShown: Binding<Bool> {
        Binding(get: { model.errorMessage != nil }, set: { if !$0 { model.errorMessage = nil } })
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            Button {
                model.importing = true
            } label: {
                Label("Add Folder", systemImage: "folder.badge.plus")
            }
        }
        ToolbarItem(placement: .automatic) {
            Button {
                Task { await model.refresh() }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
            .disabled(model.roots.isEmpty || model.progress?.isWorking == true)
        }
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            searchHeader
            if model.roots.isEmpty {
                ContentUnavailableView(
                    "No folders yet",
                    systemImage: "folder",
                    description: Text("Add a folder from Files. FSearch indexes only what you pick.")
                )
            } else if model.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                List {
                    Section {
                        ForEach(model.roots) { root in
                            Label(root.name, systemImage: "folder")
                                .swipeActions {
                                    Button(role: .destructive) {
                                        Task { await model.removeRoot(root.id) }
                                    } label: {
                                        Label("Remove", systemImage: "trash")
                                    }
                                }
                        }
                    } footer: {
                        Text("Swipe a folder to stop indexing it. Names are fuzzy. Try ext:swift, type:image, or grep:TODO.")
                    }
                }
                .id(model.generation)
            } else if model.hits.isEmpty {
                ContentUnavailableView.search(text: model.query)
            } else {
                List(model.hits, selection: $model.selectedPath) { hit in
                    HitRow(hit: hit)
                }
                .listStyle(.plain)
            }
            statusBar
        }
    }

    private var searchHeader: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Search", text: $model.query)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .onChange(of: model.query) { _, _ in
                    model.scheduleSearch()
                }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(QueryHint.all, id: \.insert) { hint in
                        Button(hint.label) {
                            insert(hint.insert)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                    }
                }
            }
        }
        .padding([.horizontal, .top])
        .padding(.bottom, 8)
    }

    private var statusBar: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.progress?.isWorking == true {
                ProgressView()
                    .progressViewStyle(.linear)
            }
            Text(statusText)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.bar)
    }

    private var statusText: String {
        let count = model.roots.count
        let folder = count == 1 ? "1 folder" : "\(count) folders"
        guard let progress = model.progress else {
            return count == 0 ? "Index is empty" : "\(folder)"
        }
        switch progress.phase {
        case .scanning:
            return "Indexing \(progress.scanned.formatted()) names…"
        case .refreshing:
            return "Refreshing \(progress.scanned.formatted()) names…"
        case .ready:
            return "\(progress.entries.formatted()) indexed · \(folder)"
        case .stopped:
            return "Stopped"
        case .idle, .unknown:
            return progress.ready ? "\(progress.entries.formatted()) indexed · \(folder)" : "Starting…"
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let hit = model.hits.first(where: { $0.path == model.selectedPath }) {
            QuickLookPreview(url: URL(fileURLWithPath: hit.path))
                .navigationTitle(hit.name)
                .navigationBarTitleDisplayMode(.inline)
                .ignoresSafeArea(edges: .bottom)
        } else {
            ContentUnavailableView(
                "No preview",
                systemImage: "doc",
                description: Text("Select a result to open it with Quick Look.")
            )
        }
    }

    private func insert(_ token: String) {
        if model.query.isEmpty || model.query.hasSuffix(" ") {
            model.query += token
        } else {
            model.query += " " + token
        }
        model.scheduleSearch()
    }
}

private struct HitRow: View {
    let hit: FSearchHit

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Image(systemName: symbol)
                    .foregroundStyle(.secondary)
                Text(hit.name)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Text("\(hit.score)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Text(hit.path)
                .font(.caption)
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.vertical, 2)
    }

    private var symbol: String {
        switch hit.kind {
        case .dir: "folder"
        case .link: "link"
        case .file, .other: "doc"
        }
    }
}

private struct QueryHint {
    var label: String
    var insert: String

    static let all: [QueryHint] = [
        QueryHint(label: "ext:", insert: "ext:"),
        QueryHint(label: "type:image", insert: "type:image"),
        QueryHint(label: "in:", insert: "in:"),
        QueryHint(label: "size:>1mb", insert: "size:>1mb"),
        QueryHint(label: "mtime:<7d", insert: "mtime:<7d"),
        QueryHint(label: "grep:", insert: "grep:"),
        QueryHint(label: "regex:", insert: "regex:"),
        QueryHint(label: "'exact", insert: "'"),
    ]
}
