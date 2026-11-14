// 一苇输入法 · macOS 外壳
// 职责只有三件：把按键交给引擎（librime）、把引擎要上屏的文字交给应用、显示候选窗。
import Cocoa
import InputMethodKit

// MARK: - 引擎

enum Engine {
    /// 中英状态全局一份：在一个应用里切了，换到别的应用也保持。
    static var englishMode = false

    static func start() {
        let fm = FileManager.default
        let shared = (Bundle.main.resourcePath ?? "") + "/rime"
        let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("Yiwei")
        let user = support.appendingPathComponent("rime").path
        let logs = support.appendingPathComponent("logs").path
        try? fm.createDirectory(atPath: user, withIntermediateDirectories: true)
        try? fm.createDirectory(atPath: logs, withIntermediateDirectories: true)
        _ = yw_start(shared, user, logs)   // 首次运行会在后台编译词库，期间按键直通
    }
}

private let kShiftMask: Int32 = 1 << 0
private let kLockMask: Int32 = 1 << 1
private let kControlMask: Int32 = 1 << 2
private let kAltMask: Int32 = 1 << 3
private let kSuperMask: Int32 = 1 << 26
private let kReleaseMask: Int32 = 1 << 30

private func rimeMask(_ f: NSEvent.ModifierFlags) -> Int32 {
    var m: Int32 = 0
    if f.contains(.shift) { m |= kShiftMask }
    if f.contains(.capsLock) { m |= kLockMask }
    if f.contains(.control) { m |= kControlMask }
    if f.contains(.option) { m |= kAltMask }
    if f.contains(.command) { m |= kSuperMask }
    return m
}

private let specialKeys: [UInt16: Int32] = [
    36: 0xff0d, 76: 0xff0d, 48: 0xff09, 49: 0x20, 51: 0xff08, 53: 0xff1b, 117: 0xffff,
    123: 0xff51, 124: 0xff53, 125: 0xff54, 126: 0xff52, 116: 0xff55, 121: 0xff56, 115: 0xff50, 119: 0xff57,
]

private func rimeKeycode(_ e: NSEvent) -> Int32? {
    if let k = specialKeys[e.keyCode] { return k }
    guard let v = e.charactersIgnoringModifiers?.unicodeScalars.first?.value, v >= 0x20, v <= 0x7e else { return nil }
    // 字母以外的键（数字、符号）在按着 Shift 时，要取“实际打出来的字符”：? ! : " 等。
    // 上面那个属性对这些键不保证带上 Shift，会把 ? 当成 /。
    let isLetter = (v >= 0x41 && v <= 0x5a) || (v >= 0x61 && v <= 0x7a)
    if !isLetter, e.modifierFlags.contains(.shift), e.modifierFlags.intersection([.control, .option, .command]).isEmpty,
       let c = e.characters?.unicodeScalars.first?.value, c >= 0x20, c <= 0x7e {
        return Int32(c)
    }
    return Int32(v)
}

private let notFound = NSRange(location: NSNotFound, length: NSNotFound)

// MARK: - 候选窗（单行，跟随系统浅色/深色）

final class CandidateView: NSView {
    var items: [String] = []
    var highlighted = 0
    var modeLabel: String?            // 只显示“中/英”提示时使用
    var onSelect: ((Int) -> Void)?
    private var frames: [NSRect] = []

    private let textFont = NSFont.systemFont(ofSize: 16)
    private let labelFont = NSFont.systemFont(ofSize: 10)
    private let padX: CGFloat = 8, padY: CGFloat = 6, gap: CGFloat = 6, itemPadX: CGFloat = 7
    let margin: CGFloat = 12           // 四周留给阴影的空白。阴影自己画，不用系统的：系统阴影每次变宽都要重算，会闪

    private var dark: Bool { UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark" }

    private(set) var contentSize = NSSize.zero     // 实际内容（白底小框加阴影留白）的大小；窗口本身比它宽

    func fittingSize() -> NSSize {
        contentSize = measure()
        return contentSize
    }
    private func measure() -> NSSize {
        frames = []
        var x = margin + padX
        let y = margin + padY
        let h = ceil(textFont.ascender - textFont.descender) + 6
        if let m = modeLabel {
            let w = ceil((m as NSString).size(withAttributes: [.font: textFont]).width) + itemPadX * 2
            frames = [NSRect(x: margin, y: margin, width: w, height: h)]     // 只有一个小块，没有外面那圈白底
            return NSSize(width: w + margin * 2, height: h + margin * 2)
        }
        for (i, s) in items.enumerated() {
            let lw = ceil(("\(i + 1)" as NSString).size(withAttributes: [.font: labelFont]).width)
            let tw = ceil((s as NSString).size(withAttributes: [.font: textFont]).width)
            let w = itemPadX + lw + 3 + tw + itemPadX
            frames.append(NSRect(x: x, y: y, width: w, height: h))
            x += w + gap
        }
        return NSSize(width: x - gap + padX + margin, height: h + (padY + margin) * 2)
    }

    override func draw(_ dirtyRect: NSRect) {
        let isDark = dark
        let bg = isDark ? NSColor(srgbRed: 0.15, green: 0.17, blue: 0.18, alpha: 1) : NSColor.white
        let fg = isDark ? NSColor(srgbRed: 0.94, green: 0.95, blue: 0.95, alpha: 1) : NSColor(srgbRed: 0.07, green: 0.07, blue: 0.07, alpha: 1)
        let hiBg = isDark ? NSColor(srgbRed: 0.95, green: 0.72, blue: 0.02, alpha: 1) : NSColor(srgbRed: 0.10, green: 0.10, blue: 0.10, alpha: 1)
        let hiFg = isDark ? NSColor(srgbRed: 0.07, green: 0.07, blue: 0.07, alpha: 1) : NSColor.white
        if let m = modeLabel, let f = frames.first {
            // 中/英提示：和选中候选一样的小块（浅色下黑底白字，深色下黄底黑字）
            NSGraphicsContext.saveGraphicsState()
            let sh = NSShadow(); sh.shadowColor = NSColor(white: 0, alpha: 0.25); sh.shadowBlurRadius = 6; sh.shadowOffset = NSSize(width: 0, height: -1)
            sh.set()
            hiBg.setFill()
            NSBezierPath(roundedRect: f, xRadius: 5, yRadius: 5).fill()
            NSGraphicsContext.restoreGraphicsState()
            let size = (m as NSString).size(withAttributes: [.font: textFont])
            (m as NSString).draw(at: NSPoint(x: f.midX - size.width / 2, y: f.midY - size.height / 2), withAttributes: [.font: textFont, .foregroundColor: hiFg])
            return
        }
        let box = NSRect(x: margin, y: margin, width: contentSize.width - margin * 2, height: contentSize.height - margin * 2)
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow(); shadow.shadowColor = NSColor(white: 0, alpha: 0.22); shadow.shadowBlurRadius = 9; shadow.shadowOffset = NSSize(width: 0, height: -2)
        shadow.set()
        bg.setFill()
        NSBezierPath(roundedRect: box, xRadius: 9, yRadius: 9).fill()
        NSGraphicsContext.restoreGraphicsState()
        NSColor(white: isDark ? 1 : 0, alpha: 0.12).setStroke()
        NSBezierPath(roundedRect: box.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9).stroke()

        for (i, s) in items.enumerated() where i < frames.count {
            let f = frames[i]
            let on = i == highlighted
            if on {
                hiBg.setFill()
                NSBezierPath(roundedRect: f, xRadius: 5, yRadius: 5).fill()
            }
            let color = on ? hiFg : fg
            let label = "\(i + 1)" as NSString
            let ls = label.size(withAttributes: [.font: labelFont])
            let ts = (s as NSString).size(withAttributes: [.font: textFont])
            label.draw(at: NSPoint(x: f.minX + itemPadX, y: f.midY - ts.height / 2 + 2),
                       withAttributes: [.font: labelFont, .foregroundColor: color.withAlphaComponent(0.55)])
            (s as NSString).draw(at: NSPoint(x: f.minX + itemPadX + ls.width + 3, y: f.midY - ts.height / 2),
                                 withAttributes: [.font: textFont, .foregroundColor: color])
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if modeLabel == nil, let i = frames.firstIndex(where: { $0.contains(p) }) { onSelect?(i) }
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

final class CandidateWindow {
    static let shared = CandidateWindow()
    private let panel: NSPanel
    private let view = CandidateView()
    private var flashToken = 0
    private var anchor: NSRect?        // 一次输入过程中候选窗钉在起始位置，不跟着应用报来的小幅变化晃

    private init() {
        panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.level = NSWindow.Level(rawValue: Int(CGShieldingWindowLevel()) - 1)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.animationBehavior = .none
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.contentView = view
    }

    private func place(near caret: NSRect) {
        let size = view.fittingSize()
        let m = view.margin
        var origin = NSPoint(x: caret.minX - m, y: caret.minY - 6 - size.height + m)
        let screen = NSScreen.screens.first(where: { $0.frame.contains(NSPoint(x: caret.midX, y: caret.midY)) }) ?? NSScreen.main
        if let vf = screen?.visibleFrame {
            if origin.x + size.width - m > vf.maxX { origin.x = vf.maxX - size.width + m - 4 }
            if origin.x + m < vf.minX { origin.x = vf.minX + 4 - m }
            if origin.y + m < vf.minY { origin.y = caret.maxY + 6 - m }       // 下面放不下就放到上面
        }
        // 窗口保持一个固定的大宽度（透明，点得穿），每按一键只重画里面的内容。
        // 之前每按一键都改窗口大小，系统来不及一次画完，边框就会抖。
        let width = max(panel.frame.width, size.width, 1200)
        let frame = NSRect(origin: NSPoint(x: origin.x.rounded(), y: origin.y.rounded()), size: NSSize(width: width, height: size.height))
        if frame != panel.frame { panel.setFrame(frame, display: false) }
        view.needsDisplay = true
        view.displayIfNeeded()
        if !panel.isVisible { panel.orderFrontRegardless() }
    }

    func show(_ items: [String], highlighted: Int, caret: NSRect, onSelect: @escaping (Int) -> Void) {
        flashToken += 1
        view.modeLabel = nil
        view.items = items
        view.highlighted = highlighted
        view.onSelect = onSelect
        // 一次输入从头到尾钉在开始的位置。有些应用每按一键报来的光标位置都不一样，跟着它就会晃。
        if anchor == nil { anchor = caret }
        place(near: anchor ?? caret)
    }

    /// 单击 Shift 后在光标旁短暂显示“中”或“英”。
    func flash(_ text: String, caret: NSRect) {
        flashToken += 1
        let token = flashToken
        anchor = nil
        view.items = []
        view.modeLabel = text
        place(near: caret)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            guard let self = self, self.flashToken == token else { return }
            self.panel.orderOut(nil)
        }
    }

    func hide() {
        flashToken += 1
        anchor = nil
        view.modeLabel = nil
        panel.orderOut(nil)
    }
}

// MARK: - 输入控制器（每个正在输入的应用一个）

@objc(YiweiInputController)
class YiweiInputController: IMKInputController {
    private var session: UInt64 = 0
    private var lastFlags: NSEvent.ModifierFlags = []
    private var hadPreedit = false
    private var shiftDownAt: TimeInterval = 0
    // 纠错候选：at 是它在候选里的位置（0 或 1），shown 是这一页一共显示几个，engineHi 是引擎自己高亮的那个
    // 候选窗里除了引擎给的候选，外壳还会自己加两种：
    //   整句——按词频切出来的整句和引擎首选不一样时，放在最前面；
    //   纠错——敲错时更可能的原意，放在引擎第一个候选后面。
    // 引擎不知道有这两个候选，所以它们的高亮、选中都由外壳自己管。
    private enum Entry: Equatable { case engine(Int), fix, sentence }
    private enum Focus { case initial, engine, entry(Int) }
    private var entries: [Entry] = []                     // 这一页从左到右依次是什么
    private var fix: (input: String, text: String)?
    private var sentence: String?
    private var focus: Int?                               // 高亮停在外壳自己加的候选上时，它在 entries 里的位置
    private var engineHi = 0                              // 引擎自己高亮的是它的第几个
    private var hasExtras: Bool { fix != nil || sentence != nil }
    private var shiftTainted = false      // 按住 Shift 期间还做了别的事（组合键、点鼠标），松开时就不算“单击”

    private func ensureSession() -> Bool {
        if yw_maintaining() { return false }
        if session == 0 {
            session = yw_session()
            if session != 0 { yw_set_ascii(session, Engine.englishMode) }
        }
        return session != 0
    }

    deinit { if session != 0 { yw_destroy(session) } }

    override func recognizedEvents(_ sender: Any!) -> Int {
        return Int(NSEvent.EventTypeMask.keyDown.rawValue | NSEvent.EventTypeMask.flagsChanged.rawValue)
    }

    override func activateServer(_ sender: Any!) {
        lastFlags = []
        if ensureSession() { yw_set_ascii(session, Engine.englishMode) }
    }

    override func deactivateServer(_ sender: Any!) {
        commitAll(sender)
        CandidateWindow.shared.hide()
    }

    override func commitComposition(_ sender: Any!) {
        commitAll(sender)
    }

    private func commitAll(_ sender: Any!) {
        guard session != 0, let client = (sender as? IMKTextInput) ?? self.client() else { return }
        if let f = focus, f < entries.count { commit(entries[f], client) }   // 高亮在哪个，上屏的就是哪个；后面如果还剩没转完的，接着照常上屏
        yw_commit_composition(session)
        apply(client)
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event = event, let client = sender as? IMKTextInput else { return false }
        guard ensureSession() else { return false }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)

        if event.type == .flagsChanged {
            let changed = flags.symmetricDifference(lastFlags)
            lastFlags = flags
            let others: NSEvent.ModifierFlags = [.control, .option, .command]
            if !changed.intersection(others).isEmpty { shiftTainted = true }
            if changed.contains(.shift) {
                // “单击 Shift”由这里判定，判定成立才把一次完整的按下、抬起交给引擎去切换。
                // 引擎自己看不到 ⇧⌃A、⇧⌘Z 这类组合键和按着 Shift 点鼠标，交给它判会误切。
                let now = ProcessInfo.processInfo.systemUptime
                if flags.contains(.shift) {
                    shiftDownAt = now
                    shiftTainted = !flags.intersection(others).isEmpty
                } else {
                    let held = now - shiftDownAt
                    for t in [CGEventType.leftMouseDown, .rightMouseDown, .otherMouseDown] where CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: t) < held { shiftTainted = true }
                    if !shiftTainted && held < 0.5 && flags.intersection(others).isEmpty {
                        let code: Int32 = event.keyCode == 60 ? 0xffe2 : 0xffe1
                        let before = yw_ascii(session)
                        _ = yw_key(session, code, kShiftMask)
                        _ = yw_key(session, code, kReleaseMask)
                        apply(client)
                        let after = yw_ascii(session)
                        if before != after {
                            Engine.englishMode = after
                            CandidateWindow.shared.flash(after ? "英" : "中", caret: caretRect(client))
                        }
                    }
                    shiftTainted = false
                }
            }
            return false
        }

        shiftTainted = true
        if flags.contains(.capsLock) {
            // 大写锁定开着：把没打完的先上屏，然后交还给系统直接打大写字母
            if hadPreedit { commitAll(sender) }
            return false
        }
        if flags.contains(.command) {
            // 输入到一半按了 ⌘ 组合键：丢掉没打完的拼音，免得它留在引擎里下一键又冒出来
            if hadPreedit { yw_clear(session); apply(client) }
            return false
        }
        guard let code = rimeKeycode(event) else { return false }
        // 候选里多了一个纠错结果时，数字键和空格要按“显示出来的顺序”算
        if hasExtras, flags.intersection([.control, .option]).isEmpty {
            let shift = flags.contains(.shift)
            if !shift, code >= 0x31 && code <= 0x39 {             // 数字键按显示出来的顺序选
                let k = Int(code - 0x31)
                if k < entries.count { commit(entries[k], client); return true }
            }
            let forward = !shift && (code == 0xff53 || code == 0xff09)                     // 右键、Tab
            let backward = (!shift && code == 0xff51) || (shift && code == 0xff09)         // 左键、Shift+Tab
            let cur = focus ?? entries.firstIndex(of: .engine(engineHi)) ?? 0
            if forward, focus != nil, cur + 1 >= entries.count { return true }             // 已经在最后一个，停住
            if forward, cur + 1 < entries.count {
                let next = cur + 1
                if case .engine(let i) = entries[next] {
                    if focus != nil && i == engineHi { apply(client, focus: .engine); return true }   // 引擎那边本来就停在它上面
                    let h = yw_key(session, code, rimeMask(flags)); apply(client, focus: .engine); return h
                }
                apply(client, focus: .entry(next)); return true
            }
            if backward {
                if focus != nil && cur == 0 { return true }                                 // 已经在最前面
                if cur > 0 {
                    let prev = cur - 1
                    if case .engine = entries[prev] {
                        if focus != nil { apply(client, focus: .engine); return true }
                    } else {
                        // 往回走到外壳自己的候选上；引擎如果不在它的第一个，先让它退一步
                        if focus == nil, engineHi > 0 { _ = yw_key(session, code, rimeMask(flags)) }
                        apply(client, focus: .entry(prev)); return true
                    }
                }
            }
            if let f = focus, f < entries.count, !forward, !backward {
                if code == 0x20 { commit(entries[f], client); return true }
                let alnum = (code >= 0x30 && code <= 0x39) || (code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a)
                // 第一页上只有 = . ] 是翻页键，' 是分隔音节；逗号、减号、[ 在第一页是标点
                let paging = [0x3d, 0x2e, 0x5d, 0x27].contains(Int(code))
                if code > 0x20 && code < 0x7f && !alnum && !paging { commit(entries[f], client) }   // 标点：先把高亮的上屏，标点再照常处理
            }
        }
        let handled = yw_key(session, code, rimeMask(flags))
        // 左右键、Tab 是在候选之间走动：高亮跟着引擎走，不要跳回最前面的整句
        let moving = code == 0xff53 || code == 0xff51 || code == 0xff09
        apply(client, focus: moving ? .engine : .initial)
        return handled
    }

    private func caretRect(_ client: IMKTextInput) -> NSRect {
        var rect = NSRect.zero
        client.attributes(forCharacterIndex: 0, lineHeightRectangle: &rect)
        if rect.origin == .zero && rect.size.height == 0 {      // 有些应用不报光标位置，就放到鼠标旁边
            let m = NSEvent.mouseLocation
            rect = NSRect(x: m.x, y: m.y - 20, width: 0, height: 20)
        }
        return rect
    }

    /// 上屏显示出来的某一个候选。
    private func commit(_ entry: Entry, _ client: IMKTextInput) {
        switch entry {
        case .engine(let i):
            _ = yw_select(session, Int32(i))
        case .fix:
            if let f = fix { _ = yw_apply_fix(session, f.input) }
        case .sentence:
            if let text = sentence {
                yw_clear(session)
                client.insertText(text, replacementRange: notFound)
                hadPreedit = false
            }
        }
        apply(client)
    }

    /// 把引擎当前的状态反映到应用和候选窗上。
    private func apply(_ client: IMKTextInput, focus want: Focus = .initial) {
        var st = YwState()
        yw_state(session, &st)
        defer { yw_free(&st) }

        if let c = st.commit {
            client.insertText(String(cString: c), replacementRange: notFound)
        }
        if let p = st.preedit {
            let text = String(cString: p)
            let bytes = Array(text.utf8)
            let cut = max(0, min(Int(st.cursor_pos), bytes.count))
            let caret = String(decoding: bytes[..<cut], as: UTF8.self).utf16.count
            let attr = NSAttributedString(string: text, attributes: [.underlineStyle: NSUnderlineStyle.single.rawValue])
            client.setMarkedText(attr, selectionRange: NSRange(location: caret, length: 0), replacementRange: notFound)
            hadPreedit = true
        } else if hadPreedit {
            client.setMarkedText("", selectionRange: NSRange(location: 0, length: 0), replacementRange: notFound)
            hadPreedit = false
        }

        var items: [String] = []
        for i in 0..<Int(st.num_candidates) {
            if let c = yw_cand(&st, Int32(i)) { items.append(String(cString: c)) }
        }
        entries = items.indices.map { .engine($0) }
        fix = nil; sentence = nil
        engineHi = Int(st.highlighted)
        if st.preedit != nil && !items.isEmpty && st.page_no == 0 {
            var fb = [CChar](repeating: 0, count: 64), tb = [CChar](repeating: 0, count: 256)
            if yw_sentence(session, &tb, 256) > 0 {
                let text = String(cString: tb)
                sentence = text; items.insert(text, at: 0); entries.insert(.sentence, at: 0)
            }
            if yw_fix(session, &fb, 64, &tb, 256) > 0 {
                let text = String(cString: tb)
                if text != sentence, let first = entries.firstIndex(of: .engine(0)) {
                    fix = (String(cString: fb), text); items.insert(text, at: first + 1); entries.insert(.fix, at: first + 1)
                }
            }
        }
        switch want {
        case .initial: focus = (sentence != nil && engineHi == 0) ? 0 : nil      // 有整句时默认高亮它
        case .engine: focus = nil
        case .entry(let i):
            if i < entries.count, case .engine = entries[i] { focus = nil } else { focus = i < entries.count ? i : nil }
        }
        let highlighted = focus ?? entries.firstIndex(of: .engine(engineHi)) ?? 0
        if items.isEmpty {
            if st.preedit == nil { CandidateWindow.shared.hide() }
        } else {
            CandidateWindow.shared.show(items, highlighted: highlighted, caret: caretRect(client)) { [weak self] index in
                guard let self = self, self.session != 0, let client = self.client(), index < self.entries.count else { return }
                self.commit(self.entries[index], client)
            }
        }
    }
}

// MARK: - 启动

// 自检用：把候选窗画到图片里（不显示在屏幕上），然后退出
if let i = CommandLine.arguments.firstIndex(of: "--snapshot"), i + 1 < CommandLine.arguments.count {
    _ = NSApplication.shared
    func shot(_ v: CandidateView, _ path: String) {
        v.frame = NSRect(origin: .zero, size: v.fittingSize())
        guard let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { return }
        v.cacheDisplay(in: v.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }
    let dir = CommandLine.arguments[i + 1]
    let a = CandidateView(); a.items = ["外框", "外", "歪", "崴", "喎"]; a.highlighted = 0; shot(a, dir + "/cand.png")
    let b = CandidateView(); b.modeLabel = "英"; shot(b, dir + "/mode.png")
    exit(0)
}

Engine.start()
let connectionName = Bundle.main.infoDictionary?["InputMethodConnectionName"] as? String ?? "Yiwei_1_Connection"
let imkServer = IMKServer(name: connectionName, bundleIdentifier: Bundle.main.bundleIdentifier)
_ = CandidateWindow.shared
NSApplication.shared.run()
