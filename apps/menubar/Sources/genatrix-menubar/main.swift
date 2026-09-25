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
// the interface, or quits, which stops the core too.
//
// "Opens the interface" means a window of this shell with a web view in it,
// showing the page the core serves, with no address bar (design 06, v0.4:
// one page, two shells; the phone's shell is its home screen). The browser
// is one menu item away for anyone who wants it.
//
// Inside Genatrix.app it is also the installer (design 09, "第一次打开就是
// 装好"): opened from the Finder, it registers the launch agent inside the
// bundle with the system, and from then on the system starts it at login
// and brings it back after a crash. When the core ends with the "erased"
// code, the shell unregisters that agent and quits for good.
//
// It is a shell and nothing more. It holds no data, no keys and no
// passwords; everything it shows it read from the core's local API.

import AppKit
import Foundation
import ServiceManagement
import WebKit

// MARK: - Arguments

struct Options {
    var genatrix: URL
    var dataDir: String?
    var port: Int = 7717
    var bind: String?
    /// Started with `--genatrix` or `--data-dir`: the developer's launch
    /// agent, not the app. The app's own installation logic stays out of it.
    var developer = false

    static func parse() -> Options {
        let exe = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        var options = Options(genatrix: exe.deletingLastPathComponent().appendingPathComponent("genatrix"))
        var args = CommandLine.arguments.dropFirst().makeIterator()
        while let arg = args.next() {
            switch arg {
            case "--genatrix": if let v = args.next() { options.genatrix = URL(fileURLWithPath: v) }; options.developer = true
            case "--data-dir": options.dataDir = args.next(); options.developer = true
            case "--port": if let v = args.next(), let p = Int(v) { options.port = p }
            case "--bind": options.bind = args.next()
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

// MARK: - The app's installation

/// The launch agent inside Genatrix.app (design 09). Registered on the first
/// open from the Finder; the system then starts the shell at login.
enum Installation {
    static let label = "xyz.dpt.genatrix.app"
    static let plist = "xyz.dpt.genatrix.app.plist"
    /// The core's exit code for "everything was deleted" (erase.rs, ERASED).
    static let erasedExit: Int32 = 64
    /// Asks a running shell to bring its window forward.
    static let openNotice = Notification.Name("xyz.dpt.genatrix.open")
    /// Set by a Finder launch that hands over to the agent, so the agent opens
    /// the window when it comes up.
    static let openOnStart = "openWindowOnNextStart"

    static var inBundle: Bool { Bundle.main.bundlePath.hasSuffix(".app") }

    static var startedBySystem: Bool {
        ProcessInfo.processInfo.environment["XPC_SERVICE_NAME"] == label
    }

    static var agent: SMAppService { SMAppService.agent(plistName: plist) }

    /// The log, opened for appending, created if it is not there yet.
    static func openLog() -> FileHandle? {
        let url = logFile
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil)
        }
        guard let handle = try? FileHandle(forWritingTo: url) else { return nil }
        handle.seekToEndOfFile()
        return handle
    }

    /// Where the core's log goes when the app runs it: launchd cannot expand
    /// a home directory in the bundle's plist, so the shell opens the file.
    static var logFile: URL {
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/Genatrix", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("genatrix.log")
    }
}

/// Whether a core already answers on the port.
func coreAnswers(port: Int) -> Bool {
    guard let url = URL(string: "http://127.0.0.1:\(port)/api/accounts") else { return false }
    var request = URLRequest(url: url)
    request.timeoutInterval = 1.5
    let done = DispatchSemaphore(value: 0)
    var ok = false
    URLSession.shared.dataTask(with: request) { data, response, _ in
        ok = (response as? HTTPURLResponse)?.statusCode == 200 && data != nil
        done.signal()
    }.resume()
    _ = done.wait(timeout: .now() + 2)
    return ok
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
        if let bind = options.bind { args += ["--bind", bind] }
        p.arguments = args
        // The core's own log goes wherever ours goes: launchd's log file for
        // the developer's agent, ~/Library/Logs/Genatrix for the app.
        if Installation.inBundle && !options.developer, let log = Installation.openLog() {
            p.standardOutput = log
            p.standardError = log
        } else {
            p.standardOutput = FileHandle.standardOutput
            p.standardError = FileHandle.standardError
        }
        p.terminationHandler = { [weak self] proc in
            guard let self = self, !self.quitting else { return }
            if proc.terminationStatus == Installation.erasedExit {
                // Everything was deleted: nothing may bring the core back.
                self.quitting = true
                DispatchQueue.main.async {
                    if Installation.inBundle { try? Installation.agent.unregister() }
                    NSApp.terminate(nil)
                }
                return
            }
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

// MARK: - The window

/// One window with the page in it. Made on first open, kept across closes,
/// reloaded when the core it showed has gone away and come back.
final class Page: NSObject, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    private var window: NSWindow?
    private var web: WKWebView?
    private var failed = false

    func open(_ url: URL) {
        if window == nil { build(url) }
        guard let window = window, let web = web else { return }
        if failed || web.url == nil {
            failed = false
            web.load(URLRequest(url: url))
        }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    private func build(_ url: URL) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1080, height: 760),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = "Genatrix"
        window.isReleasedWhenClosed = false
        window.setFrameAutosaveName("GenatrixPage")
        if window.frame.width < 400 { window.center() }
        window.delegate = self
        let configuration = WKWebViewConfiguration()
        let web = WKWebView(frame: window.contentView!.bounds, configuration: configuration)
        web.autoresizingMask = [.width, .height]
        web.navigationDelegate = self
        web.uiDelegate = self
        window.contentView?.addSubview(web)
        web.load(URLRequest(url: url))
        self.window = window
        self.web = web
    }

    // The core was not there, or stopped mid-page: remember, so the next
    // open loads again instead of showing the error for good.
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { failed = true }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { failed = true }

    // A file input on the page, such as installing an agent from a package:
    // the system's open panel, since a web view has none of its own.
    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping ([URL]?) -> Void) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        if let window = window {
            panel.beginSheetModal(for: window) { answer in
                completionHandler(answer == .OK ? panel.urls : nil)
            }
        } else {
            completionHandler(panel.runModal() == .OK ? panel.urls : nil)
        }
    }

    // Links that leave the core's page go to the browser; the window shows
    // Genatrix and nothing else. A download, such as an agent's exported
    // data, goes to the browser too, which saves it where downloads go.
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        if navigationAction.shouldPerformDownload, let url = navigationAction.request.url {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
            return
        }
        if let url = navigationAction.request.url, let host = url.host,
           host != "127.0.0.1", host != "localhost", navigationAction.navigationType == .linkActivated {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
            return
        }
        decisionHandler(.allow)
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
    private let page = Page()

    init(options: Options) {
        self.options = options
        self.core = Core(options: options)
    }

    /// What the menu says about the installation, when there is something.
    private var installNote: String?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        if Installation.inBundle && !options.developer && !install() { return }
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.toolTip = "Genatrix"
        show(.syncing)
        rebuildMenu()
        // A Finder launch that handed over, or a second open of the app: bring
        // the window forward.
        DistributedNotificationCenter.default().addObserver(
            forName: Installation.openNotice, object: nil, queue: .main
        ) { [weak self] _ in self?.openInterface() }
        core.start()
        if UserDefaults.standard.bool(forKey: Installation.openOnStart) || !Installation.startedBySystem {
            UserDefaults.standard.set(false, forKey: Installation.openOnStart)
            // The core needs a moment; the window reloads until it answers.
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in self?.openInterface() }
        }
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

    /// Design 09, "第一次打开就是装好". Returns whether this process should go
    /// on to run the core itself.
    ///
    /// Started by the system: this is the installed agent; run, unless another
    /// Genatrix already holds the port (a Finder launch that ran on while the
    /// user had not yet allowed the login item), in which case leave quietly
    /// and start at the next login.
    ///
    /// Opened from the Finder: if Genatrix is already running, ask it to show
    /// its window and leave. Otherwise register the agent; once the system has
    /// it, the system starts it, so leave the running to it and ask it to open
    /// the window. If the system wants the user's approval first, or will not
    /// register it, run here meanwhile, and say so in the menu.
    private func install() -> Bool {
        if Installation.startedBySystem {
            if coreAnswers(port: options.port) {
                NSApp.terminate(nil)
                return false
            }
            return true
        }
        if coreAnswers(port: options.port) {
            DistributedNotificationCenter.default().postNotificationName(
                Installation.openNotice, object: nil, userInfo: nil, deliverImmediately: true)
            NSApp.terminate(nil)
            return false
        }
        let agent = Installation.agent
        if agent.status != .enabled {
            do {
                try agent.register()
            } catch {
                FileHandle.standardError.write("genatrix-menubar: could not register the login item: \(error)\n".data(using: .utf8)!)
            }
        }
        switch agent.status {
        case .enabled:
            UserDefaults.standard.set(true, forKey: Installation.openOnStart)
            NSApp.terminate(nil)
            return false
        case .requiresApproval:
            installNote = "Allow Genatrix in Login Items to keep it running after you log out"
            SMAppService.openSystemSettingsLoginItems()
            return true
        default:
            installNote = "Genatrix could not add itself to Login Items; it runs until you log out"
            return true
        }
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
        if let note = installNote {
            let line = NSMenuItem(title: note, action: #selector(openLoginItems), keyEquivalent: "")
            line.target = self
            menu.addItem(line)
        }
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
        let browser = NSMenuItem(title: "Open in Browser", action: #selector(openInBrowser), keyEquivalent: "")
        browser.target = self
        menu.addItem(browser)
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

    @objc func openInterface() {
        if let url = URL(string: "http://127.0.0.1:\(options.port)/") {
            page.open(url)
        }
    }

    @objc private func openLoginItems() {
        SMAppService.openSystemSettingsLoginItems()
    }

    @objc private func openInBrowser() {
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
