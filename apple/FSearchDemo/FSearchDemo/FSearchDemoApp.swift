import SwiftUI

@main
struct FSearchDemoApp: App {
    @State private var model = SearchModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .task { await model.bootstrap() }
                .onChange(of: scenePhase) { _, phase in
                    if phase == .active {
                        Task { await model.refresh() }
                    }
                }
        }
    }
}
