import Foundation

/// What the running process says about itself, in one line for the menu bar.
///
/// Read from the process's OWN endpoint — the node's status page, the server's
/// health — rather than inferred from the fact that it is running. A process
/// can be up and not linked, up with no outputs, up and refusing to render;
/// "the icon is green because `fork` returned" is the reassurance that gets
/// somebody to Sunday without checking.
struct Health {
    enum State {
        /// The child is not running at all.
        case down
        /// Running, but its own endpoint is not answering yet.
        case starting
        /// Running and answering, with a sentence for the menu.
        case up(String)
        /// Running, answering, and something is ON AIR. Worth its own state:
        /// it is the one condition where quitting is destructive.
        case onAir(String)
    }

    static func poll(_ urlString: String, running: Bool) async -> State {
        guard running else { return .down }
        guard let url = URL(string: urlString) else { return .starting }
        var req = URLRequest(url: url)
        req.timeoutInterval = 1.5
        req.cachePolicy = .reloadIgnoringLocalCacheData
        guard let (data, resp) = try? await URLSession.shared.data(for: req),
              let http = resp as? HTTPURLResponse, http.statusCode == 200,
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            return .starting
        }
        return summarise(json)
    }

    /// One sentence for whichever process answered.
    ///
    /// The two shapes are told apart by what they contain rather than by which
    /// app is asking: a launcher that had to be told which endpoint it was
    /// reading would be a second place the two apps differ.
    private static func summarise(_ json: [String: Any]) -> State {
        // A node: `{ outputs: [ { id, fps, late_frames, layers: [...] } ] }`
        if let outputs = json["outputs"] as? [[String: Any]] {
            if outputs.isEmpty { return .up("no outputs") }
            let onAir = outputs.contains { out in
                (out["layers"] as? [[String: Any]] ?? []).contains { $0["occupant"] is [String: Any] }
            }
            let late = outputs.reduce(0) { $0 + (($1["late_frames"] as? Int) ?? 0) }
            let fps = (outputs.first?["fps"] as? Double).map { String(format: "%.2f", $0) } ?? "—"
            let word = outputs.count == 1 ? "output" : "outputs"
            var line = "\(outputs.count) \(word) · \(fps) fps"
            // Late frames are the number that matters and the only one worth
            // the space when it is not zero.
            if late > 0 { line += " · \(late) late" }
            return onAir ? .onAir(line) : .up(line)
        }
        // The server: `{ ok: true, milestone: "M6", ... }`
        if json["ok"] is Bool {
            if let stream = json["stream"] as? [String: Any],
               let conns = stream["connections"] as? Int {
                let word = conns == 1 ? "console" : "consoles"
                return .up("serving · \(conns) \(word)")
            }
            return .up("serving")
        }
        return .up("answering")
    }
}
