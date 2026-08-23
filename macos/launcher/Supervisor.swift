import Foundation

/// Runs the real process, keeps it running, and writes down what happened.
///
/// # Why a supervisor and not a LaunchAgent
///
/// A LaunchAgent would restart the child too. It would also put the thing an
/// operator has to reason about in a plist they cannot see, started by a system
/// they cannot ask. Here the app IS the supervisor: the menu bar says whether
/// the child is up, the log is one click away, and quitting the app stops the
/// child — which is what everybody expects and what a LaunchAgent specifically
/// does not do.
///
/// # Restarting is not optional
///
/// ADR 0009 says a node that loses the server keeps playing. The same reasoning
/// one level down: a node that crashes at 10:15 on a Sunday must come back
/// without anybody knowing where the binary lives. It restarts with a backoff,
/// because a process that cannot start at all must not become a fork bomb in a
/// booth.
final class Supervisor {
    private let config: LauncherConfig
    private var process: Process?
    private var restarts = 0
    private var backoff: TimeInterval = 0.5
    private var stopping = false
    private let queue = DispatchQueue(label: "prodmesh.supervisor")

    /// Called on the main queue whenever the child starts or stops.
    var onChange: (() -> Void)?

    private(set) var lastExit: String?
    var running: Bool { process?.isRunning ?? false }
    var restartCount: Int { restarts }

    init(config: LauncherConfig) {
        self.config = config
    }

    var logURL: URL {
        config.supportDir.appendingPathComponent("\(config.executable).log")
    }

    func start() {
        queue.async { [weak self] in self?.spawn() }
    }

    /// Stop the child and mean it.
    ///
    /// SIGTERM first, so a node can close its outputs and a server can finish
    /// its transaction; SIGKILL only if it will not go. Quitting the app must
    /// not leave an orphan holding a DeckLink — the next launch would then fail
    /// to open the device with no explanation.
    func stop() {
        stopping = true
        guard let p = process, p.isRunning else { return }
        p.terminate()
        let deadline = Date().addingTimeInterval(5)
        while p.isRunning && Date() < deadline {
            usleep(50_000)
        }
        if p.isRunning {
            kill(p.processIdentifier, SIGKILL)
        }
    }

    func restart() {
        queue.async { [weak self] in
            guard let self else { return }
            self.stopping = false
            self.backoff = 0.5
            if let p = self.process, p.isRunning {
                p.terminate()
            } else {
                self.spawn()
            }
        }
    }

    private func spawn() {
        guard !stopping else { return }
        let support = config.supportDir
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)

        guard let exe = Bundle.main.url(forResource: config.executable, withExtension: nil) else {
            lastExit = "\(config.executable) is not in this bundle"
            DispatchQueue.main.async { self.onChange?() }
            return
        }

        let p = Process()
        p.executableURL = exe
        p.arguments = config.arguments.map(config.expand)
        var env = ProcessInfo.processInfo.environment
        for (k, v) in config.environment {
            env[k] = config.expand(v)
        }
        p.environment = env
        // The bundle's Resources, so a child that resolves anything relative —
        // the server's own `dist/` — finds it where it was installed.
        p.currentDirectoryURL = exe.deletingLastPathComponent()

        // One log file, appended, so a crash at 10:15 is still readable at 11.
        // Truncating on launch would destroy the evidence of the thing that
        // caused the launch.
        let log = logURL
        if !FileManager.default.fileExists(atPath: log.path) {
            FileManager.default.createFile(atPath: log.path, contents: nil)
        }
        if let handle = try? FileHandle(forWritingTo: log) {
            handle.seekToEndOfFile()
            p.standardOutput = handle
            p.standardError = handle
        }

        p.terminationHandler = { [weak self] proc in
            guard let self else { return }
            self.queue.async {
                self.lastExit = "exited with \(proc.terminationStatus) at \(Self.stamp())"
                DispatchQueue.main.async { self.onChange?() }
                guard !self.stopping else { return }
                self.restarts += 1
                // Backoff, capped. A binary that cannot start at all must not
                // become a spin loop in a booth.
                let wait = self.backoff
                self.backoff = min(self.backoff * 2, 30)
                self.queue.asyncAfter(deadline: .now() + wait) { self.spawn() }
            }
        }

        do {
            try p.run()
            process = p
            // A child that has stayed up is a child that started cleanly; the
            // backoff should not remember a failure from an hour ago.
            queue.asyncAfter(deadline: .now() + 30) { [weak self] in
                if self?.process?.isRunning == true { self?.backoff = 0.5 }
            }
            DispatchQueue.main.async { self.onChange?() }
        } catch {
            lastExit = "would not start: \(error.localizedDescription)"
            DispatchQueue.main.async { self.onChange?() }
        }
    }

    private static func stamp() -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss"
        return f.string(from: Date())
    }
}
