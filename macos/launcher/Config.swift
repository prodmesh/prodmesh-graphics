import Foundation

/// What this bundle supervises, read from `Launcher.json` in its Resources.
///
/// Both apps ship the SAME binary and differ only in this file. Two launchers
/// with two codebases would drift — one would learn to restart on crash, or to
/// write its log somewhere findable, and the other would not, and nobody would
/// notice until the night the wrong one was needed.
struct LauncherConfig: Decodable {
    /// "ProdMesh Director" — the app, and the Application Support folder.
    let name: String
    /// "DIRECTOR" — under the lockup on the splash.
    let subtitle: String
    /// Executable inside `Contents/Resources`, run with the bundle as its cwd.
    let executable: String
    /// Arguments. `{support}` expands to this app's Application Support folder.
    let arguments: [String]
    /// Environment on top of the inherited one. `{support}` expands the same way.
    let environment: [String: String]
    /// Polled every couple of seconds for the menu's status line.
    let healthURL: String
    /// What "Open…" opens — the browser console, or the node's status page.
    let openURL: String
    let openTitle: String

    static func load() -> LauncherConfig {
        guard let url = Bundle.main.url(forResource: "Launcher", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let cfg = try? JSONDecoder().decode(LauncherConfig.self, from: data)
        else {
            // A bundle without its own configuration cannot be repaired at
            // runtime, and guessing would start the wrong process.
            fatalError("Launcher.json is missing or malformed — this bundle is broken.")
        }
        return cfg
    }

    /// Where this app's database, logs and node identity live.
    ///
    /// `~/Library/Application Support/<name>`, NOT inside the bundle: an app
    /// that writes to itself cannot be replaced by dragging a new one in, and
    /// on a booth machine the thing you must never lose is the state.
    var supportDir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent(name, isDirectory: true)
    }

    func expand(_ s: String) -> String {
        s.replacingOccurrences(of: "{support}", with: supportDir.path)
    }
}
