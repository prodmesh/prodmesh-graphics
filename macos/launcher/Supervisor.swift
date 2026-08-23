import Darwin
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

    /// Where the child's pid is written, so a launcher that was KILLED can be
    /// cleaned up after by the next one.
    private var pidFile: URL { config.supportDir.appendingPathComponent(".child.pid") }

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
        queue.async { [weak self] in
            self?.reapOrphan()
            self?.spawn()
        }
    }

    /// Stop a child left behind by a launcher that did not get to say goodbye.
    ///
    /// SIGKILL, a crash, a force quit: the parent never runs another line, so
    /// nothing on this side can stop the child. It keeps the DeckLink, the
    /// ports and the NDI name, and the next launch meets a device that will not
    /// open with no explanation anywhere. The pid on disk is how the NEXT
    /// launcher cleans up after the last one.
    ///
    /// Checked by name as well as by pid, because pids are reused and killing
    /// whatever now holds one would be far worse than leaving an orphan.
    private func reapOrphan() {
        guard let text = try? String(contentsOf: pidFile, encoding: .utf8),
              let pid = pid_t(text.trimmingCharacters(in: .whitespacesAndNewlines)),
              pid > 0, kill(pid, 0) == 0
        else { return }

        var name = [CChar](repeating: 0, count: 4096)
        let n = proc_pidpath(pid, &name, UInt32(name.count))
        let path = n > 0 ? String(cString: name) : ""
        guard path.hasSuffix("/" + config.executable) else { return }

        tracing("a previous \(config.executable) (pid \(pid)) was still running; stopping it")
        kill(pid, SIGTERM)
        let deadline = Date().addingTimeInterval(5)
        while kill(pid, 0) == 0 && Date() < deadline { usleep(50_000) }
        if kill(pid, 0) == 0 { kill(pid, SIGKILL) }
    }

    private func tracing(_ line: String) {
        guard let data = "[launcher] \(line)\n".data(using: .utf8),
              let h = try? FileHandle(forWritingTo: logURL) else { return }
        h.seekToEndOfFile()
        h.write(data)
        try? h.close()
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
        try? FileManager.default.removeItem(at: pidFile)
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
            try? String(p.processIdentifier).write(to: pidFile, atomically: true, encoding: .utf8)
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
