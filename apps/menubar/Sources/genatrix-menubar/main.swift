// genatrix-menubar: the icon in the menu bar, and the process behind it.
//
// Design: docs/design/09-install-recover-migrate.md, "常驻" and "状态与恢复";
// docs/design/06-interface.md.
//
// After installation Genatrix is a background program that starts at login
// and shows one icon in the menu bar. This is that program. It starts the
// core (`genatrix serve`), keeps it running, and asks it every few seconds
// how each account is doing. The icon has the four looks the design names:
// normal, syncing, needs you, error. Clicking it lists the accounts, opens
// the interface in the browser, or quits, which stops the core too.
//
// It is a shell and nothing more. It holds no data, no keys and no
// passwords; everything it shows it read from the core's local API.

import AppKit
import Foundation

// MARK: - Arguments

struct Options {
    var genatrix: URL
    var dataDir: String?
    var port: Int = 7717

    static func parse() -> Options {
        let exe = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        var options = Options(genatrix: exe.deletingLastPathComponent().appendingPathComponent("genatrix"))
        var args = CommandLine.arguments.dropFirst().makeIterator()
        while let arg = args.next() {
            switch arg {
            case "--genatrix": if let v = args.next() { options.genatrix = URL(fileURLWithPath: v) }
            case "--data-dir": options.dataDir = args.next()
            case "--port": if let v = args.next(), let p = Int(v) { options.port = p }
            default: break
            }
        }
        return options
    }
}

// MARK: - What the core says

struct AccountsReply: Decodable {
    struct Account: Decodable {
        struct Sync: Decodable { let state: String }
        let address: String
        let sync: Sync
        let text: String
    }
    let accounts: [Account]
}

/// The four looks of the icon.
enum Look {
    case normal, syncing, needsYou, error

    var symbol: String {
        switch self {
        case .normal: return "circle"
        case .syncing: return "arrow.triangle.2.circlepath"
        case .needsYou: return "exclamationmark.circle"
        case .error: return "xmark.circle"
        }
    }

    var word: String {
        switch self {
        case .normal: return "up to date"
        case .syncing: return "syncing"
        case .needsYou: return "needs you"
        case .error: return "error"
        }
    }

    /// The worst of the accounts is the look of the whole.
    static func of(_ accounts: [AccountsReply.Account]) -> Look {
        if accounts.contains(where: { $0.sync.state == "stopped" }) { return .error }
        if accounts.contains(where: { $0.sync.state == "needs_login" }) { return .needsYou }
        if accounts.contains(where: { ["starting", "connecting", "backfilling", "retrying"].contains($0.sync.state) }) {
            return .syncing
        }
        return .normal
    }
}

// MARK: - The core process

/// Runs `genatrix serve` and starts it again when it stops, until told to quit.
final class Core {
    private let options: Options
    private var process: Process?
    private var quitting = false

    init(options: Options) { self.options = options }

    func start() {
        guard !quitting else { return }
        let p = Process()
        p.executableURL = options.genatrix
        var args: [String] = []
        if let dir = options.dataDir { args += ["--data-dir", dir] }
        args += ["serve", "--port", String(options.port)]
        p.arguments = args
        // The core's own log goes wherever ours goes: launchd's log file.
        p.standardOutput = FileHandle.standardOutput
        p.standardError = FileHandle.standardError
        p.terminationHandler = { [weak self] proc in
            guard let self = self, !self.quitting else { return }
            FileHandle.standardError.write("genatrix-menubar: the core stopped (\(proc.terminationStatus)); starting it again\n".data(using: .utf8)!)
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self.start() }
        }
        do {
            try p.run()
            process = p
        } catch {
            FileHandle.standardError.write("genatrix-menubar: could not start \(options.genatrix.path): \(error)\n".data(using: .utf8)!)
            DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in self?.start() }
        }
    }

    /// Stop for good. The core gets SIGTERM and a moment to close its files.
    func quit() {
        quitting = true
        guard let p = process, p.isRunning else { return }
        p.terminate()
        p.waitUntilExit()
    }
}

// MARK: - The menu bar

final class Shell: NSObject, NSApplicationDelegate {
    private let options: Options
    private let core: Core
    private var item: NSStatusItem!
    private var timer: Timer?
    private var accounts: [AccountsReply.Account] = []
    private var reachable = false

    init(options: Options) {
        self.options = options
        self.core = Core(options: options)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.toolTip = "Genatrix"
        show(.syncing)
        rebuildMenu()
        core.start()
        poll()
        timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in self?.poll() }
        // Sleep suspends the connectors; a wake is worth asking about at once.
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.poll() }
    }

    func applicationWillTerminate(_ notification: Notification) {
        core.quit()
    }

    private func show(_ look: Look) {
        guard let button = item.button else { return }
        let image = NSImage(systemSymbolName: look.symbol, accessibilityDescription: "Genatrix: \(look.word)")
        image?.isTemplate = true
        button.image = image
        button.toolTip = "Genatrix: \(look.word)"
    }

    private func poll() {
        guard let url = URL(string: "http://127.0.0.1:\(options.port)/api/accounts") else { return }
        var request = URLRequest(url: url)
        request.timeoutInterval = 3
        URLSession.shared.dataTask(with: request) { [weak self] data, _, _ in
            let reply = data.flatMap { try? JSONDecoder().decode(AccountsReply.self, from: $0) }
            DispatchQueue.main.async {
                guard let self = self else { return }
                self.reachable = reply != nil
                self.accounts = reply?.accounts ?? []
                self.show(self.reachable ? Look.of(self.accounts) : .syncing)
                self.rebuildMenu()
            }
        }.resume()
    }

    private func rebuildMenu() {
        let menu = NSMenu()
        let headline = reachable ? "Genatrix · \(Look.of(accounts).word)" : "Genatrix · starting"
        menu.addItem(disabled(headline))
        menu.addItem(.separator())
        if accounts.isEmpty {
            menu.addItem(disabled(reachable ? "No accounts yet" : "Waiting for the core"))
        }
        for account in accounts {
            let line = NSMenuItem(title: account.address, action: nil, keyEquivalent: "")
            line.isEnabled = false
            let detail = NSMenuItem(title: "    " + account.text, action: nil, keyEquivalent: "")
            detail.isEnabled = false
            menu.addItem(line)
            menu.addItem(detail)
        }
        menu.addItem(.separator())
        let open = NSMenuItem(title: "Open Genatrix", action: #selector(openInterface), keyEquivalent: "o")
        open.target = self
        menu.addItem(open)
        menu.addItem(.separator())
        let quit = NSMenuItem(title: "Quit Genatrix", action: #selector(quitAll), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        item.menu = menu
    }

    private func disabled(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        return item
    }

    @objc private func openInterface() {
        if let url = URL(string: "http://127.0.0.1:\(options.port)/") {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func quitAll() {
        core.quit()
        NSApp.terminate(nil)
    }
}

let options = Options.parse()
let app = NSApplication.shared
let shell = Shell(options: options)
app.delegate = shell
// SIGTERM from launchd (uninstall, logout) should take the core down too.
signal(SIGTERM) { _ in DispatchQueue.main.async { NSApp.terminate(nil) } }
app.run()
