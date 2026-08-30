import Kithara
import SwiftUI

@main
@MainActor
struct KitharaDemoApp: App {
    private let viewModel: Result<PlayerViewModel, Error>?

    init() {
        Kithara.initLogging(level: .debug)
        viewModel = Self.isHostingTests ? nil : Result { try PlayerViewModel() }
    }

    #if os(macOS)
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    #endif

    /// Hosted unit tests need this process and its audio session, not its UI.
    /// Building `PlayerView` would start a second player against the demo's own
    /// stream, which competes with the tests for the shared asset cache.
    private static var isHostingTests: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
            || NSClassFromString("XCTestCase") != nil
    }

    var body: some Scene {
        WindowGroup {
            if Self.isHostingTests {
                Color.clear
            } else if let viewModel {
                Group {
                    switch viewModel {
                    case .success(let viewModel):
                        PlayerView(viewModel: viewModel)
                    case .failure(let error):
                        Text("Kithara initialization failed: \(error)")
                    }
                }
                #if os(macOS)
                .onAppear {
                    // CLI-launched executables (not .app bundles) don't
                    // automatically become the active app on macOS,
                    // so keyboard events (including Cmd+V) are not delivered.
                    NSApplication.shared.setActivationPolicy(.regular)
                    NSApplication.shared.activate(ignoringOtherApps: true)
                }
                #endif
            } else {
                Color.clear
            }
        }
        #if os(macOS)
        .commands {
            TextEditingCommands()
        }
        // Quit the process when the last window is closed.
        .defaultSize(width: 520, height: 760)
        #endif
    }
}

#if os(macOS)
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_: NSApplication) -> Bool {
        true
    }
}
#endif
