import AppKit
import ServiceManagement

/// The menu-bar app both ProdMesh Director and ProdMesh Playout are.
///
/// # Why a menu-bar item and not a window
///
/// The real interface is the browser console; a second one would be two places
/// that must agree about what is on air, and they would not. What the machine
/// itself needs to say is smaller than a window: is this thing up, is anything
/// on air, where is the log, how do I open the console. That is a menu.
///
/// `LSUIElement` keeps it out of the Dock and the app switcher, which is right
/// for something that boots with the machine and is never chosen deliberately.
final class Launcher: NSObject, NSApplicationDelegate {
    private let config = LauncherConfig.load()
    private lazy var supervisor = Supervisor(config: config)
    private var item: NSStatusItem!
    private var status: Health.State = .down
    private var timer: Timer?
    private var splash: NSWindow?
    /// The child is up but still looking for something on the network — which
    /// is when the Local Network permission is worth naming.
    private var isHunting = false
    /// Held for the life of the app; a released source stops firing.
    private var signalSources: [DispatchSourceSignal] = []

    func applicationDidFinishLaunching(_: Notification) {
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = icon(onAir: false)
        item.button?.image?.isTemplate = true
        item.menu = NSMenu()
        item.menu?.delegate = self

        supervisor.onChange = { [weak self] in self?.refresh() }
        installSignalHandlers()
        supervisor.start()
        showSplash()

        // Two seconds: fast enough that a menu opened after a failure tells the
        // truth, slow enough to be free.
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            self?.refresh()
        }
        refresh()
    }

    /// Stop the child on the way out.
    ///
    /// Quitting the app that owns a process and leaving the process behind is
    /// how a DeckLink ends up held by an orphan, and the next launch fails to
    /// open the device with no explanation anywhere.
    func applicationWillTerminate(_: Notification) {
        supervisor.stop()
    }

    /// The same, for the ways an app is ended that are NOT "quit".
    ///
    /// `applicationWillTerminate` does not run for a signal, and a booth
    /// machine ends processes with signals all the time: a logout, a shutdown,
    /// `pkill`, a script tidying up. Measured — four orphaned `pmg-node`
    /// processes after four `pkill`s of the launcher, every one still holding
    /// its ports.
    ///
    /// `SIG_IGN` first so the default action does not kill the process before
    /// the dispatch source ever runs.
    private func installSignalHandlers() {
        for sig in [SIGTERM, SIGINT, SIGHUP] {
            signal(sig, SIG_IGN)
            let src = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            src.setEventHandler { [weak self] in
                self?.supervisor.stop()
                exit(0)
            }
            src.resume()
            signalSources.append(src)
        }
    }

    private func refresh() {
        Task { @MainActor in
            let s = await Health.poll(config.healthURL, running: supervisor.running)
            self.status = s
            if case .starting = s { self.isHunting = supervisor.running } else { self.isHunting = false }
            if case .onAir = s {
                self.item.button?.image = self.icon(onAir: true)
                self.item.button?.image?.isTemplate = false
            } else {
                self.item.button?.image = self.icon(onAir: false)
                self.item.button?.image?.isTemplate = true
            }
        }
    }

    /// A dot. Red ONLY when something is on air, and never for a failure —
    /// ADR 0020's rule, and it matters more here than anywhere: this icon sits
    /// beside the clock all service, and an eye that has learned red means
    /// "problem" will not read it as "live".
    private func icon(onAir: Bool) -> NSImage {
        let size = NSSize(width: 14, height: 14)
        let image = NSImage(size: size)
        image.lockFocus()
        let rect = NSRect(x: 3, y: 3, width: 8, height: 8)
        (onAir ? NSColor.systemRed : NSColor.labelColor).setFill()
        NSBezierPath(ovalIn: rect).fill()
        image.unlockFocus()
        return image
    }

    // MARK: - The splash

    /// Shown once, briefly, on launch.
    ///
    /// A background app that starts with the machine gives no other sign it
    /// exists. Three seconds of the lockup is the whole of it: it never blocks,
    /// it takes no focus, and clicking dismisses it.
    private func showSplash() {
        guard let art = Bundle.main.image(forResource: "Splash") else { return }
        // Sized from the ART's own aspect, not from a number typed here. A
        // hardcoded box either letterboxes the picture or crops it the day
        // somebody redraws it at a different shape.
        let wide: CGFloat = 720
        let aspect = art.size.height > 0 ? art.size.height / art.size.width : 0.5
        let size = NSSize(width: wide, height: (wide * aspect).rounded())
        let w = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        w.isOpaque = false
        w.backgroundColor = .clear
        w.level = .floating
        w.ignoresMouseEvents = false
        w.hasShadow = true
        let view = NSImageView(frame: NSRect(origin: .zero, size: size))
        view.image = art
        view.imageScaling = .scaleProportionallyUpOrDown
        w.contentView = view
        w.center()
        // Fades in rather than appearing. A machine booting into a service puts
        // several of these on screen at once, and a hard cut reads as a glitch.
        w.alphaValue = 0
        // No focus: a machine booting into a service must not have this steal
        // the keyboard from whatever the operator opened next.
        w.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.25
            w.animator().alphaValue = 1
        }
        splash = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            self?.dismissSplash()
        }
    }

    private func dismissSplash() {
        guard let w = splash else { return }
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.35
            w.animator().alphaValue = 0
        } completionHandler: {
            w.orderOut(nil)
        }
        splash = nil
    }

    // MARK: - Menu actions

    @objc private func openConsole() {
        if let url = URL(string: config.openURL) { NSWorkspace.shared.open(url) }
    }

    @objc private func openLog() {
        NSWorkspace.shared.selectFile(
            supervisor.logURL.path,
            inFileViewerRootedAtPath: config.supportDir.path
        )
    }

    @objc private func openData() {
        NSWorkspace.shared.open(config.supportDir)
    }

    @objc private func restart() {
        supervisor.restart()
    }

    /// Start with the machine.
    ///
    /// `SMAppService` rather than a LaunchAgent plist: it is one API call, the
    /// user can see and revoke it in System Settings, and there is no file to
    /// leave behind when the app is dragged to the Trash.
    @objc private func toggleLogin() {
        let service = SMAppService.mainApp
        do {
            if service.status == .enabled {
                try service.unregister()
            } else {
                try service.register()
            }
        } catch {
            let a = NSAlert()
            a.messageText = "Could not change whether \(config.name) starts with this Mac."
            a.informativeText = error.localizedDescription
            a.runModal()
        }
    }

    @objc private func quit() {
        if case .onAir = status {
            // The one destructive thing this menu can do. Naming it is the
            // whole guard: an operator quitting the wrong menu-bar item mid
            // service takes the graphics off air.
            let a = NSAlert()
            a.messageText = "Something is on air."
            a.informativeText =
                "Quitting \(config.name) stops it, and whatever is showing goes away."
            a.addButton(withTitle: "Quit anyway")
            a.addButton(withTitle: "Cancel")
            a.alertStyle = .warning
            if a.runModal() != .alertFirstButtonReturn { return }
        }
        NSApp.terminate(nil)
    }
}

extension Launcher: NSMenuDelegate {
    func menuWillOpen(_ menu: NSMenu) {
        menu.removeAllItems()

        let line: String
        switch status {
        case .down: line = supervisor.running ? "Starting…" : "Not running"
        case .starting: line = "Starting…"
        case .up(let s): line = s
        case .onAir(let s): line = "ON AIR · \(s)"
        }
        let head = NSMenuItem(title: "\(config.name) — \(line)", action: nil, keyEquivalent: "")
        head.isEnabled = false
        menu.addItem(head)

        // The failure that looks exactly like a network with no server on it.
        //
        // macOS Local Network Privacy denies a NEW app silently: the browse
        // returns nothing rather than an error, so a node hunting for a
        // Director reports "none found" whether the permission is missing or
        // the Director really is absent. It is per responsible app, so a fresh
        // install starts denied — and the log is the only place that says so.
        // This is the sentence that turns a dead end into a checkbox.
        if !supervisor.running || isHunting {
            let p = NSMenuItem(
                title: "If nothing is found: System Settings › Privacy & Security › Local Network",
                action: nil, keyEquivalent: ""
            )
            p.isEnabled = false
            menu.addItem(p)
        }

        // Only when there is something to say. A line reading "restarts: 0" is
        // noise; one reading "restarted 4 times" is the whole story.
        if supervisor.restartCount > 0 {
            let n = supervisor.restartCount
            let r = NSMenuItem(
                title: "Restarted \(n) time\(n == 1 ? "" : "s") — \(supervisor.lastExit ?? "")",
                action: nil, keyEquivalent: ""
            )
            r.isEnabled = false
            menu.addItem(r)
        } else if !supervisor.running, let why = supervisor.lastExit {
            let r = NSMenuItem(title: why, action: nil, keyEquivalent: "")
            r.isEnabled = false
            menu.addItem(r)
        }

        menu.addItem(.separator())
        add(menu, config.openTitle, #selector(openConsole))
        add(menu, "Show the log…", #selector(openLog))
        add(menu, "Show data folder…", #selector(openData))
        menu.addItem(.separator())

        let login = add(menu, "Start with this Mac", #selector(toggleLogin))
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off

        add(menu, "Restart \(config.name)", #selector(restart))
        add(menu, "Quit \(config.name)", #selector(quit))
    }

    @discardableResult
    private func add(_ menu: NSMenu, _ title: String, _ action: Selector) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: action, keyEquivalent: "")
        i.target = self
        menu.addItem(i)
        return i
    }
}

let app = NSApplication.shared
let delegate = Launcher()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
