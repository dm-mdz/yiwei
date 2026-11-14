// 一苇 · 菜单栏外壳：图标、下拉界面、配置、快捷键、截屏
import Cocoa
import Carbon
import ScreenCaptureKit
import ServiceManagement

// MARK: 配置（一个文本文件，可手改）

struct Config {
    static let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Yiwei")
    static let file = dir.appendingPathComponent("config.toml")
    var screenshot = true
    var hotkey = "shift+ctrl+a"
    var saveDir = "~/Desktop"
    var capsNoDelay = false
    var share = false
    var receiveDir = "~/Downloads/一苇"

    static func load() -> Config {
        var c = Config()
        guard let text = try? String(contentsOf: file, encoding: .utf8) else { c.save(); return c }
        for line in text.split(separator: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("#") || !t.contains("=") { continue }
            let parts = t.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            guard parts.count == 2 else { continue }
            let v = parts[1].trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            switch parts[0] {
            case "screenshot": c.screenshot = (v == "true")
            case "screenshot_hotkey": c.hotkey = v.lowercased()
            case "screenshot_save_dir": c.saveDir = v
            case "caps_lock_no_delay": c.capsNoDelay = (v == "true")
            case "key_mouse_share": c.share = (v == "true")
            case "receive_dir": c.receiveDir = v
            default: break
            }
        }
        return c
    }
    func save() {
        try? FileManager.default.createDirectory(at: Config.dir, withIntermediateDirectories: true)
        let text = """
        # 一苇的设置。平时在菜单栏的下拉菜单里改就行，这个文件由它写入，一般不用手改。

        # 截图：是否开启、快捷键、保存文件的位置
        screenshot = \(screenshot)
        screenshot_hotkey = "\(hotkey)"
        screenshot_save_dir = "\(saveDir)"

        # 键鼠共享：一套键盘鼠标控制几台电脑（配对过的电脑记在同一文件夹的 share.json 里）
        key_mouse_share = \(share)
        # 别的电脑发来的文件放在哪
        receive_dir = "\(receiveDir)"

        # 大写锁定键不延迟：去掉系统对大写锁定键的防误触延迟（按得快也生效）
        caps_lock_no_delay = \(capsNoDelay)

        """
        try? text.write(to: Config.file, atomically: true, encoding: .utf8)
    }
}

// MARK: 全局快捷键（向系统登记，不监听键盘）

final class Hotkey {
    private var ref: EventHotKeyRef?
    private static var installed = false
    private static var actions: [UInt32: () -> Void] = [:]
    private let id: UInt32

    static let keyCodes: [String: UInt32] = ["a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46]

    init?(spec: String, id: UInt32, action: @escaping () -> Void) {
        self.id = id
        var mods: UInt32 = 0
        var key: UInt32?
        for part in spec.lowercased().split(separator: "+").map({ $0.trimmingCharacters(in: .whitespaces) }) {
            switch part {
            case "shift": mods |= UInt32(shiftKey)
            case "ctrl", "control": mods |= UInt32(controlKey)
            case "alt", "option", "opt": mods |= UInt32(optionKey)
            case "cmd", "command": mods |= UInt32(cmdKey)
            default: key = Hotkey.keyCodes[part]
            }
        }
        guard let code = key, mods != 0 else { return nil }
        guard register(code, mods) else { return nil }
        Hotkey.actions[id] = action
    }

    init?(code: UInt32, mods: UInt32, id: UInt32, action: @escaping () -> Void) {
        self.id = id
        guard register(code, mods) else { return nil }
        Hotkey.actions[id] = action
    }

    private func register(_ code: UInt32, _ mods: UInt32) -> Bool {
        if !Hotkey.installed {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
                var hk = EventHotKeyID()
                GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &hk)
                Hotkey.actions[hk.id]?()
                return noErr
            }, 1, &spec, nil, nil)
            Hotkey.installed = true
        }
        let hkID = EventHotKeyID(signature: OSType(0x5949_5749), id: id)
        return RegisterEventHotKey(code, mods, hkID, GetApplicationEventTarget(), 0, &ref) == noErr
    }
    deinit {
        if let r = ref { UnregisterEventHotKey(r) }
        Hotkey.actions[id] = nil
    }

    static func display(_ spec: String) -> String {
        spec.lowercased().split(separator: "+").map { p -> String in
            switch p { case "shift": return "⇧"; case "ctrl", "control": return "⌃"; case "alt", "option", "opt": return "⌥"; case "cmd", "command": return "⌘"; default: return p.uppercased() }
        }.joined()
    }
}

// MARK: 大写锁定键不延迟
// 系统对苹果键盘（以及自称苹果键盘的第三方键盘）的大写锁定键有防误触延迟，按得快会被忽略。
// 这里用系统自带的 hidutil 把延迟设为 0。键盘重连、睡眠唤醒后系统会恢复原样，所以启动和唤醒时各设一次。

enum CapsLock {
    static func removeDelay() {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/hidutil")
        p.arguments = ["property", "--set", "{\"CapsLockDelayOverride\":0}"]
        p.standardOutput = FileHandle.nullDevice; p.standardError = FileHandle.nullDevice
        try? p.run()
    }
}

// MARK: 截屏

enum Capture {
    static func all() async throws -> [(NSScreen, CGImage)] {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        var out: [(NSScreen, CGImage)] = []
        for display in content.displays {
            guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID) == display.displayID }) else { continue }
            let cfg = SCStreamConfiguration()
            cfg.width = Int(CGFloat(display.width) * screen.backingScaleFactor)
            cfg.height = Int(CGFloat(display.height) * screen.backingScaleFactor)
            cfg.showsCursor = false
            let image = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display: display, excludingWindows: []), configuration: cfg)
            out.append((screen, image))
        }
        return out
    }
}

// MARK: 菜单栏图标（单色模板：最高一根实色，其余略淡）

func statusIcon() -> NSImage {
    let img = NSImage(named: "status") ?? NSImage(size: NSSize(width: 18, height: 18))
    img.size = NSSize(width: 18, height: 18)
    img.isTemplate = true
    return img
}

// MARK: 下拉界面

final class SwitchRow: NSView {
    let title = NSTextField(labelWithString: "")
    let sub = NSTextField(labelWithString: "")
    let toggle = NSSwitch()
    var onToggle: ((Bool) -> Void)?

    init(_ t: String) {
        super.init(frame: .zero)
        title.stringValue = t; title.font = .systemFont(ofSize: 13.5, weight: .semibold)
        sub.font = .systemFont(ofSize: 12); sub.textColor = .secondaryLabelColor; sub.lineBreakMode = .byWordWrapping; sub.maximumNumberOfLines = 2; sub.preferredMaxLayoutWidth = 226
        toggle.controlSize = .small; toggle.target = self; toggle.action = #selector(flip)
        let text = NSStackView(views: [title, sub]); text.orientation = .vertical; text.alignment = .leading; text.spacing = 1
        let row = NSStackView(views: [text, toggle]); row.orientation = .horizontal; row.distribution = .fill
        row.edgeInsets = NSEdgeInsets(top: 7, left: 8, bottom: 7, right: 8)
        text.setContentHuggingPriority(.defaultLow, for: .horizontal); toggle.setContentHuggingPriority(.required, for: .horizontal)
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([row.leadingAnchor.constraint(equalTo: leadingAnchor), row.trailingAnchor.constraint(equalTo: trailingAnchor), row.topAnchor.constraint(equalTo: topAnchor), row.bottomAnchor.constraint(equalTo: bottomAnchor)])
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func flip() { onToggle?(toggle.state == .on) }
}

final class ItemRow: NSView {
    let label = NSTextField(labelWithString: "")
    let detail = NSTextField(labelWithString: "")
    var onClick: (() -> Void)?
    init(_ t: String, _ d: String = "", onClick: @escaping () -> Void) {
        super.init(frame: .zero)
        self.onClick = onClick
        wantsLayer = true; layer?.cornerRadius = 6
        label.stringValue = t; label.font = .systemFont(ofSize: 13)
        detail.stringValue = d; detail.font = .systemFont(ofSize: 13); detail.textColor = .secondaryLabelColor
        let row = NSStackView(views: [label, detail]); row.orientation = .horizontal; row.distribution = .equalSpacing
        row.edgeInsets = NSEdgeInsets(top: 5, left: 8, bottom: 5, right: 8)
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([row.leadingAnchor.constraint(equalTo: leadingAnchor), row.trailingAnchor.constraint(equalTo: trailingAnchor), row.topAnchor.constraint(equalTo: topAnchor), row.bottomAnchor.constraint(equalTo: bottomAnchor)])
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }
    required init?(coder: NSCoder) { fatalError() }
    override func mouseEntered(with event: NSEvent) { layer?.backgroundColor = NSColor.labelColor.withAlphaComponent(0.08).cgColor }
    override func mouseExited(with event: NSEvent) { layer?.backgroundColor = NSColor.clear.cgColor }
    override func mouseUp(with event: NSEvent) { layer?.backgroundColor = NSColor.clear.cgColor; onClick?() }
}

final class PanelController: NSViewController {
    unowned let app: AppDelegate
    private let shot = SwitchRow("截图")
    let sharePanel = SharePanel()
    private let autostart = SwitchRow("开机时自动启动")
    private let caps = SwitchRow("大写锁定键不延迟")
    private var hotkeyRow: ItemRow!
    private var dirRow: ItemRow!
    private var recorder: Any?          // 正在等用户按新快捷键时的按键监听
    private var capsJustOff = false

    init(app: AppDelegate) { self.app = app; super.init(nibName: nil, bundle: nil) }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let stack = DropStack(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 2
        stack.edgeInsets = NSEdgeInsets(top: 8, left: 8, bottom: 8, right: 8)
        func add(_ v: NSView) { stack.addArrangedSubview(v); v.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -16).isActive = true }
        add(shot)
        hotkeyRow = ItemRow("截图快捷键") { [unowned self] in self.toggleRecording() }
        dirRow = ItemRow("截图保存到") { [unowned self] in self.chooseDir() }
        add(hotkeyRow); add(dirRow)
        sharePanel.views.forEach(add)
        sharePanel.onResize = { [unowned self] in self.refresh() }
        let sep = NSBox(); sep.boxType = .separator; add(sep)
        add(caps)
        add(autostart)
        add(ItemRow("退出一苇") { NSApp.terminate(nil) })
        stack.widthAnchor.constraint(equalToConstant: 300).isActive = true
        view = stack

        shot.onToggle = { [unowned self] on in self.app.config = Config.load(); self.app.config.screenshot = on; self.app.config.save(); self.app.applyScreenshot(); self.refresh() }
        sharePanel.toggle.onToggle = { [unowned self] on in
            self.app.config = Config.load(); self.app.config.share = on; self.app.config.save()
            if on { Share.shared.start() } else { Share.shared.stop() }
            self.refresh()
        }
        autostart.onToggle = { [unowned self] on in self.setAutostart(on) }
        caps.onToggle = { [unowned self] on in
            self.app.config = Config.load(); self.app.config.capsNoDelay = on; self.app.config.save()
            if on { CapsLock.removeDelay() }
            self.capsJustOff = !on
            self.refresh()
        }
    }
    override func viewWillAppear() { super.viewWillAppear(); refresh() }

    override func viewWillDisappear() { super.viewWillDisappear(); stopRecording() }

    // 点“截图快捷键”后直接按新的组合键来改
    private func toggleRecording() {
        if recorder != nil { stopRecording(); refresh(); return }
        app.suspendHotkey()                 // 录的时候先停用旧快捷键，免得一按就截图
        hotkeyRow.detail.stringValue = "请按新的快捷键（Esc 取消）"
        recorder = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [unowned self] e in
            if e.keyCode == 53 { self.stopRecording(); self.refresh(); return nil }
            let f = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            var parts: [String] = []
            if f.contains(.shift) { parts.append("shift") }
            if f.contains(.control) { parts.append("ctrl") }
            if f.contains(.option) { parts.append("alt") }
            if f.contains(.command) { parts.append("cmd") }
            guard let key = Hotkey.keyCodes.first(where: { $0.value == UInt32(e.keyCode) })?.key else {
                self.hotkeyRow.detail.stringValue = "只能用字母或数字键，请重按"; return nil
            }
            guard parts.contains(where: { $0 != "shift" }) else {
                self.hotkeyRow.detail.stringValue = "要带上 ⌃、⌥ 或 ⌘，请重按"; return nil
            }
            let old = self.app.config.hotkey, spec = (parts + [key]).joined(separator: "+")
            self.stopRecording(resume: false)
            self.app.config = Config.load(); self.app.config.hotkey = spec; self.app.applyScreenshot()
            if self.app.hotkeyOK { self.app.config.save(); self.refresh() }
            else {
                self.app.config.hotkey = old; self.app.applyScreenshot(); self.refresh()
                self.hotkeyRow.detail.stringValue = Hotkey.display(spec) + " 被占用，没改"
            }
            return nil
        }
    }
    private func stopRecording(resume: Bool = true) {
        guard let r = recorder else { return }
        NSEvent.removeMonitor(r); recorder = nil
        if resume { app.applyScreenshot() }
    }
    private func chooseDir() {
        let p = NSOpenPanel()
        p.canChooseDirectories = true; p.canChooseFiles = false; p.canCreateDirectories = true; p.prompt = "保存到这里"
        p.directoryURL = URL(fileURLWithPath: NSString(string: app.config.saveDir).expandingTildeInPath)
        NSApp.activate(ignoringOtherApps: true)
        if p.runModal() == .OK, let url = p.url {
            app.config = Config.load(); app.config.saveDir = (url.path as NSString).abbreviatingWithTildeInPath; app.config.save()
        }
        refresh()
    }

    func refresh() {
        let on = app.config.screenshot
        shot.toggle.state = on ? .on : .off
        shot.sub.stringValue = on ? "回车复制" : "已关闭"
        hotkeyRow.isHidden = !on; dirRow.isHidden = !on
        if recorder == nil { hotkeyRow.detail.stringValue = Hotkey.display(app.config.hotkey) + (app.hotkeyOK ? "" : "（被占用）") }
        dirRow.detail.stringValue = FileManager.default.displayName(atPath: NSString(string: app.config.saveDir).expandingTildeInPath)
        sharePanel.refresh()
        caps.toggle.state = app.config.capsNoDelay ? .on : .off
        caps.sub.stringValue = app.config.capsNoDelay ? "按一下就生效" : (capsJustOff ? "已关闭，重启后恢复原样" : "去掉防误触延迟")
        autostart.toggle.state = SMAppService.mainApp.status == .enabled ? .on : .off
        autostart.sub.isHidden = autostart.sub.stringValue.isEmpty
        // 行数变了，下拉框跟着变高矮。外框被系统按当前高度钉住，直接问它“合适多高”只会变大不会变小：自己把每一行加起来
        view.layoutSubtreeIfNeeded()
        let stack = view as! NSStackView
        let rows = stack.arrangedSubviews.filter { !$0.isHidden }
        let height = stack.edgeInsets.top + stack.edgeInsets.bottom + rows.reduce(0) { $0 + $1.fittingSize.height } + stack.spacing * CGFloat(max(0, rows.count - 1))
        preferredContentSize = NSSize(width: 300, height: ceil(height))
    }
    private func setAutostart(_ on: Bool) {
        do { if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() } ; autostart.sub.stringValue = "" }
        catch { autostart.sub.stringValue = "设置失败，请在系统设置“登录项”里添加" }
        refresh()
    }
}

// MARK: 应用

final class AppDelegate: NSObject, NSApplicationDelegate, NSPopoverDelegate {
    /// 下拉框关了（再点一下菜单栏图标）：取消选中，下次打开恢复成点别处就收起
    func popoverDidClose(_ notification: Notification) { Share.shared.selected = nil }
    var config = Config.load()
    private var statusItem: NSStatusItem!
    private let popover = NSPopover()
    private var hotkey: Hotkey?
    var hotkeyOK: Bool { hotkey != nil }
    private var capturing = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        UserDefaults.standard.set(250, forKey: "NSInitialToolTipDelay")
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.image = statusIcon()
        statusItem.button?.toolTip = "一苇"
        statusItem.button?.target = self
        statusItem.button?.action = #selector(togglePanel)
        // 把文件拖到菜单栏图标上：下拉自动打开，再拖到要发给的那台电脑的卡片上
        if let b = statusItem.button {
            let drop = StatusDropView(frame: b.bounds); drop.autoresizingMask = [.width, .height]
            drop.onClick = { [weak self] in self?.togglePanel() }
            drop.onDragEnter = { [weak self] in if self?.popover.isShown == false { self?.togglePanel() } }
            b.addSubview(drop)
        }
        popover.behavior = .transient
        popover.delegate = self
        // 左键选中一台电脑：下拉框固定住，点别处不收起，好从访达把文件拖进来；取消选中就恢复
        Share.shared.onPin = { [weak self] pinned in
            self?.popover.behavior = pinned ? .applicationDefined : .transient
            (self?.popover.contentViewController as? PanelController)?.refresh()
        }
        let panel = PanelController(app: self)
        popover.contentViewController = panel
        Share.shared.onChange = { [weak panel] in if panel?.isViewLoaded == true { panel?.refresh() } }
        if config.share { Share.shared.start() }
        applyScreenshot()
        if config.capsNoDelay { CapsLock.removeDelay() }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            // 唤醒后蓝牙键盘要过几秒才连回来
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) { if self?.config.capsNoDelay == true { CapsLock.removeDelay() } }
        }
    }

    func applicationWillTerminate(_ notification: Notification) { Share.shared.stop() }   // 鼠标在别的电脑上时也要先放回来

    func closePanel() { if popover.isShown { popover.performClose(nil) } }

    @objc func togglePanel() {
        guard let b = statusItem.button else { return }
        if popover.isShown { popover.performClose(nil) } else { popover.show(relativeTo: b.bounds, of: b, preferredEdge: .minY); NSApp.activate(ignoringOtherApps: true) }
    }

    func suspendHotkey() { hotkey = nil }

    /// 截图关闭时不登记快捷键，什么都不占。
    func applyScreenshot() {
        hotkey = nil
        if config.screenshot { hotkey = Hotkey(spec: config.hotkey, id: 1) { [weak self] in self?.startScreenshot() } }
    }

    func startScreenshot() {
        guard ShotSession.current == nil, !capturing else { return }
        capturing = true
        Task { @MainActor in
            defer { self.capturing = false }
            do {
                let shots = try await Capture.all()
                guard !shots.isEmpty else { return }
                ShotSession.current = ShotSession(shots: shots, saveDir: self.config.saveDir)
            } catch {
                self.explainPermission()
            }
        }
    }

    private func explainPermission() {
        let a = NSAlert()
        a.messageText = "截图需要“屏幕录制”权限"
        a.informativeText = "请在系统设置的“隐私与安全性 → 屏幕与系统录音”里，把“一苇”打开，然后重新按快捷键。系统大约每月会再确认一次，这是系统的规定。"
        a.addButton(withTitle: "打开系统设置"); a.addButton(withTitle: "以后再说")
        NSApp.activate(ignoringOtherApps: true)
        if a.runModal() == .alertFirstButtonReturn, let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") { NSWorkspace.shared.open(url) }
    }
}

/// 盖在菜单栏图标上的一层：点一下照常开关下拉；拖着文件进来时打开下拉
final class StatusDropView: NSView {
    var onClick: (() -> Void)?
    var onDragEnter: (() -> Void)?
    override init(frame: NSRect) { super.init(frame: frame); registerForDraggedTypes([.fileURL]) }
    required init?(coder: NSCoder) { fatalError() }
    override func mouseDown(with event: NSEvent) { onClick?() }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { onDragEnter?(); return .copy }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        // 直接松在图标上：只连着一台就发给它，否则等用户拖到卡片上
        let urls = (sender.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        let online = Share.shared.machines.values.filter(\.online)
        guard online.count == 1, !urls.isEmpty else { return false }
        Share.shared.send(urls, to: online[0].id)
        return true
    }
}

// MARK: 自检用：把界面画到图片里（不显示在屏幕上）

func renderPNG(_ view: NSView, to path: String) {
    let w = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    w.contentView?.addSubview(view)
    view.layoutSubtreeIfNeeded()
    guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
    view.cacheDisplay(in: view.bounds, to: rep)
    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
}

func runSnapshots(_ dir: String) {
    let app = AppDelegate()
    let panel = PanelController(app: app)
    _ = panel.view; panel.refresh()
    panel.view.frame = NSRect(origin: .zero, size: panel.view.fittingSize)
    renderPNG(panel.view, to: dir + "/panel.png")
    // 键鼠共享打开后的样子：收起的小图，展开的大图（含一台待加入）
    Share.shared.showDemo(found: true)
    Share.shared.demoTransfer()
    for (expanded, name) in [(false, "panel_share.png"), (true, "panel_board.png")] {
        let p = PanelController(app: app)
        _ = p.view; p.sharePanel.expanded = expanded; p.refresh()
        p.view.frame = NSRect(origin: .zero, size: p.view.fittingSize)
        renderPNG(p.view, to: dir + "/" + name)
    }
    // 先展开再收起：应当和一开始收起时一样高
    let back = PanelController(app: app)
    _ = back.view; back.sharePanel.expanded = true; back.refresh(); back.sharePanel.expanded = false; back.refresh()
    back.view.frame = NSRect(origin: .zero, size: back.preferredContentSize)
    renderPNG(back.view, to: dir + "/panel_collapsed.png")
    print("展开再收起后的高度：", back.preferredContentSize.height, "（应和一开始收起时一样）")

    let size = NSSize(width: 900, height: 520)
    let img = NSImage(size: size, flipped: false) { r in
        NSGradient(colors: [NSColor(white: 0.95, alpha: 1), NSColor(srgbRed: 0.80, green: 0.86, blue: 0.90, alpha: 1)])?.draw(in: r, angle: 60)
        ("这是一段用来检查截图效果的文字 Sample text 1234567890" as NSString).draw(at: NSPoint(x: 150, y: 300), withAttributes: [.font: NSFont.systemFont(ofSize: 20), .foregroundColor: NSColor.black])
        return true
    }
    guard let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
    let v = OverlayView(image: cg, frame: NSRect(origin: .zero, size: size))
    v.sel = NSRect(x: 120, y: 170, width: 560, height: 250)
    let red = NSColor(srgbRed: 0.90, green: 0.21, blue: 0.17, alpha: 1)
    v.marks = [Mark(tool: .rect, points: [NSPoint(x: 150, y: 285), NSPoint(x: 420, y: 335)], color: red, width: 3),
               Mark(tool: .arrow, points: [NSPoint(x: 600, y: 220), NSPoint(x: 470, y: 300)], color: red, width: 3),
               Mark(tool: .pen, points: (0...40).map { NSPoint(x: 180 + CGFloat($0) * 6, y: 220 + sin(CGFloat($0) / 4) * 14) }, color: ywGold, width: 3),
               Mark(tool: .mosaic, points: [NSPoint(x: 430, y: 290), NSPoint(x: 640, y: 330)], color: red, width: 3),
               Mark(tool: .text, points: [NSPoint(x: 150, y: 400)], color: red, width: 18, text: "文字标注 Text 123")]
    v.tool = .rect
    renderPNG(v, to: dir + "/overlay.png")
    if let out = v.exportImage() { try? NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir + "/export.png")) }
    // 提取文字：认出来的字垫淡蓝，选中的一段深一些，工具条左边多“全选、复制”
    let doc = NSImage(size: size, flipped: false) { r in
        NSColor.white.setFill(); r.fill()
        let lines = ["会议纪要 2026-11-13", "1. 键鼠共享本周内三台真机验收完", "2. Release v0.16 notes: clipboard follows pointer", "3. 联系人 someone@example.com，电话 010-5555-0199"]
        for (i, l) in lines.enumerated() { (l as NSString).draw(at: NSPoint(x: 150, y: 370 - CGFloat(i) * 40), withAttributes: [.font: NSFont.systemFont(ofSize: 18), .foregroundColor: NSColor.black]) }
        return true
    }
    guard let cg2 = doc.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
    let v2 = OverlayView(image: cg2, frame: NSRect(origin: .zero, size: size))
    v2.sel = NSRect(x: 120, y: 200, width: 600, height: 220)
    v2.ocrForSnapshot(pick: 16...30)
    renderPNG(v2, to: dir + "/overlay_ocr.png")
}
