// 一苇 · 键鼠共享（macOS 外壳）
// 核心（core/，Rust）管连网、入组、谁在控制谁；这里管和系统打交道的部分：
//   · 存取这台电脑的身份、所在的组、组里的电脑、摆法（~/Library/Application Support/Yiwei/share.json，只有自己能读）
//   · 监听本机键鼠（事件监听，需要“辅助功能”权限），报给核心；鼠标穿到别的电脑时把本机鼠标藏起来、停住，输入不再给本机
//   · 把别的电脑发来的输入注入本机
//   · 下拉界面里的位置图、校验码、发现的电脑、“允许加入吗”的确认
import Cocoa
import ApplicationServices
import Carbon
import UserNotifications

/// 记一行运行记录（~/Library/Logs/Yiwei/share.log）
func slog(_ t: String) { ywk_log(t) }

// MARK: 存档

struct ShareStore: Codable {
    struct Peer: Codable { var id: String; var key: String; var name: String; var os: Int }
    struct Cell: Codable { var id: String; var x: Int; var y: Int }
    var id = "", priv = "", pub = ""
    /// 这台电脑所在的组（核心给的一行字，原样存、原样传回去）。没有：第一次用，或者是从“两台核对 6 位数字”的旧版升上来的
    var group: String?
    var peers: [Peer] = []
    var rev: UInt64 = 0
    var cells: [Cell] = []
    /// “鼠标被控切回”开没开（没有这一项 = 关）
    var takeBack: Bool?

    static let file = Config.dir.appendingPathComponent("share.json")
    static func load() -> ShareStore {
        if let d = try? Data(contentsOf: file) {
            if let s = try? JSONDecoder().decode(ShareStore.self, from: d), s.id.count == 32, s.priv.count == 64 { return s }
            // 读不懂（坏了，或者以后格式变了）：先留一份，别直接覆盖，原来的还能找回来
            let backup = file.deletingLastPathComponent().appendingPathComponent("share.json.坏了-\(Int(Date().timeIntervalSince1970))")
            try? FileManager.default.moveItem(at: file, to: backup)
        }
        var s = ShareStore()
        s.id = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        if let p = ywk_new_identity() {
            let parts = String(cString: p).split(separator: " ").map(String.init)
            ywk_free(p)
            if parts.count == 2 { s.priv = parts[0]; s.pub = parts[1] }
        }
        s.save()
        return s
    }
    func save() {
        try? FileManager.default.createDirectory(at: Config.dir, withIntermediateDirectories: true)
        guard let d = try? JSONEncoder().encode(self) else { return }
        let path = ShareStore.file.path
        // 里面有私钥：一开始就建成只有自己能读写的文件，之后原地改写，权限不变
        if !FileManager.default.fileExists(atPath: path) { FileManager.default.createFile(atPath: path, contents: nil, attributes: [.posixPermissions: 0o600]) }
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
        try? d.write(to: ShareStore.file)
    }
}

// MARK: 屏幕：核心只认一块“屏幕”，这里把所有显示器合起来的外框报给它（多块屏幕的内侧边不会误穿）

enum Screens {
    /// 所有屏幕合起来的外框，用系统的全局坐标（左上角原点，往下为正）
    static func union() -> CGRect {
        guard let main = NSScreen.screens.first else { return CGRect(x: 0, y: 0, width: 1440, height: 900) }
        var r = CGRect.null
        for s in NSScreen.screens {
            let f = s.frame
            r = r.union(CGRect(x: f.minX, y: main.frame.maxY - f.maxY, width: f.width, height: f.height))
        }
        return r
    }
}

// MARK: 藏鼠标

enum Cursor {
    private static var hidden = false
    private static var allowed = false
    static func hide() {
        guard !hidden else { return }
        if !allowed { allowed = true; allowInBackground() }
        CGDisplayHideCursor(CGMainDisplayID()); hidden = true
    }
    static func show() {
        guard hidden else { return }
        CGDisplayShowCursor(CGMainDisplayID()); hidden = false
    }
    /// 系统默认只让最前面的程序藏鼠标。一苇在后台，要打开这个开关（同类软件都这么做；找不到就算了，只是鼠标不藏）
    private static func allowInBackground() {
        typealias Conn = @convention(c) () -> Int32
        typealias SetProp = @convention(c) (Int32, Int32, CFString, CFTypeRef) -> Int32
        let any = UnsafeMutableRawPointer(bitPattern: -2)          // RTLD_DEFAULT
        guard let a = dlsym(any, "_CGSDefaultConnection"), let b = dlsym(any, "CGSSetConnectionProperty") else { return }
        let conn = unsafeBitCast(a, to: Conn.self)()
        _ = unsafeBitCast(b, to: SetProp.self)(conn, conn, "SetsCursorInBackground" as CFString, kCFBooleanTrue)
    }
}

// MARK: 修饰键

private let injectedMark: Int64 = 0x5957_4B31                       // 自己注入的事件带这个记号，监听时跳过
/// 修饰键的键码 → （通用标志位，左右分开的标志位）
private let modifierFlags: [CGKeyCode: (UInt64, UInt64)] = [
    56: (CGEventFlags.maskShift.rawValue, 0x02), 60: (CGEventFlags.maskShift.rawValue, 0x04),
    59: (CGEventFlags.maskControl.rawValue, 0x01), 62: (CGEventFlags.maskControl.rawValue, 0x2000),
    58: (CGEventFlags.maskAlternate.rawValue, 0x20), 61: (CGEventFlags.maskAlternate.rawValue, 0x40),
    55: (CGEventFlags.maskCommand.rawValue, 0x08), 54: (CGEventFlags.maskCommand.rawValue, 0x10),
]
private let capsLock: CGKeyCode = 57
/// 左右两个同类的修饰键
private func partner(_ k: CGKeyCode) -> CGKeyCode? { [56: 60, 60: 56, 59: 62, 62: 59, 58: 61, 61: 58, 55: 54, 54: 55][k] }
/// 给 ywk_sync_mods 用：每个修饰键（核心里的第几位，通用标志位，这一边的标志位，左右两边的标志位）
private let modSync: [(bit: UInt32, any: UInt64, side: UInt64, pair: UInt64)] = modifierFlags.compactMap { k, f in
    let u = ywk_usage_from_mac(Int32(k))
    guard u >= 0xE0, u <= 0xE7 else { return nil }
    return (UInt32(1) << UInt32(u - 0xE0), f.0, f.1, f.1 | (partner(k).flatMap { modifierFlags[$0]?.1 } ?? 0))
}
/// 事件上带的标志位里，现在真按着哪些修饰键。只带通用标志位、不分左右的（虚拟键盘）：两边都算按着
private func heldMods(_ flags: UInt64) -> UInt32 {
    var m: UInt32 = 0
    for e in modSync where flags & e.any != 0 && (flags & e.pair == 0 || flags & e.side != 0) { m |= e.bit }
    return m
}

// MARK: 注入别的电脑发来的输入（在核心的线程上被调用）

final class Injector {
    private let lock = NSLock()
    private let src: CGEventSource? = {
        let s = CGEventSource(stateID: .hidSystemState)
        s?.localEventsSuppressionInterval = 0                    // 被控制时本机自己的键鼠不被压住
        return s
    }()
    private var origin = CGPoint.zero
    func setOrigin(_ p: CGPoint) { lock.lock(); origin = p; lock.unlock() }
    /// 对方的鼠标走了：how 0 正常走了、1 被请回去了（换了一台来控制，或者本机的鼠标自己穿出去了）、2 对方掉线了
    var onLeave: ((Int32) -> Void)?
    /// 对方的鼠标进来了
    var onEnter: (() -> Void)?
    private var pos = CGPoint.zero
    private var buttons = Set<Int32>()
    private var keys = Set<CGKeyCode>()
    private var mods = Set<CGKeyCode>()
    private var caps = false
    private var guestOn = false
    /// 现在是不是有别的电脑的指针在本机上
    var hosting: Bool { lock.lock(); defer { lock.unlock() }; return guestOn }
    private var wheel = (x: 0.0, y: 0.0)
    private var lastDown = (button: Int32(0), at: 0.0, p: CGPoint.zero, count: Int64(0))

    private var flags: CGEventFlags {
        var f: UInt64 = 0
        for m in mods { if let b = modifierFlags[m] { f |= b.0 | b.1 } }
        if caps { f |= CGEventFlags.maskAlphaShift.rawValue }
        return CGEventFlags(rawValue: f)
    }
    private func post(_ e: CGEvent?) {
        guard let e else { return }
        e.setIntegerValueField(.eventSourceUserData, value: injectedMark)
        e.post(tap: .cghidEventTap)
    }

    func apply(_ kind: Int32, _ a: Int32, _ b: Int32, _ c: Int32, _ d: Int32) {
        lock.lock(); defer { lock.unlock() }
        switch kind {
        case 1: move(to: CGPoint(x: origin.x + CGFloat(a), y: origin.y + CGFloat(b)))
        case 2: scroll(a, b)
        case 3: key(a, b == 1)
        case 4: button(a, b == 1, at: CGPoint(x: origin.x + CGFloat(c), y: origin.y + CGFloat(d)))
        case 5:
            guestOn = true
            let f = onEnter; DispatchQueue.main.async { f?() }
            caps = c & 16 != 0                                                             // 大写锁定跟着对方的键盘（它的灯亮着，打出来就是大写）
            for (bit, k) in [(1, CGKeyCode(56)), (2, 59), (4, 58), (8, 55)] where c & Int32(bit) != 0 && !mods.contains(k) { modifier(k, true) }
        case 6:
            guestOn = false
            releaseAllLocked()
            let f = onLeave; DispatchQueue.main.async { f?(a) }
        default: break
        }
    }
    func releaseAll() { lock.lock(); releaseAllLocked(); lock.unlock() }

    private func releaseAllLocked() {
        for k in keys { let e = CGEvent(keyboardEventSource: src, virtualKey: k, keyDown: false); e?.flags = flags; post(e) }
        keys.removeAll()
        for b in buttons { post(mouse(b, down: false)) }
        buttons.removeAll()
        for k in mods { modifier(k, false) }
    }

    private func move(to p: CGPoint) {
        let type: CGEventType = buttons.contains(1) ? .leftMouseDragged : buttons.contains(2) ? .rightMouseDragged : buttons.isEmpty ? .mouseMoved : .otherMouseDragged
        let btn: CGMouseButton = buttons.contains(1) ? .left : buttons.contains(2) ? .right : .center
        guard let e = CGEvent(mouseEventSource: src, mouseType: type, mouseCursorPosition: p, mouseButton: btn) else { return }
        e.setDoubleValueField(.mouseEventDeltaX, value: Double(p.x - pos.x))
        e.setDoubleValueField(.mouseEventDeltaY, value: Double(p.y - pos.y))
        e.flags = flags
        pos = p
        post(e)
    }
    /// 120 = 一行，这里一行按 10 个点算
    private func scroll(_ dx: Int32, _ dy: Int32) {
        wheel.x += Double(dx) / 12; wheel.y += Double(dy) / 12
        let ix = Int32(wheel.x.rounded(.towardZero)), iy = Int32(wheel.y.rounded(.towardZero))
        wheel.x -= Double(ix); wheel.y -= Double(iy)
        guard ix != 0 || iy != 0 else { return }
        post(CGEvent(scrollWheelEvent2Source: src, units: .pixel, wheelCount: 2, wheel1: iy, wheel2: -ix, wheel3: 0))
    }
    private func modifier(_ k: CGKeyCode, _ down: Bool) {
        if down { mods.insert(k) } else { mods.remove(k) }
        guard let e = CGEvent(keyboardEventSource: src, virtualKey: k, keyDown: down) else { return }
        e.type = .flagsChanged; e.flags = flags
        post(e)
    }
    private func key(_ usage: Int32, _ down: Bool) {
        let code = ywk_mac_from_usage(usage)
        guard code >= 0 else { return }
        let k = CGKeyCode(code)
        if k == capsLock {
            guard down else { return }
            caps.toggle()
            let e = CGEvent(keyboardEventSource: src, virtualKey: k, keyDown: true); e?.type = .flagsChanged; e?.flags = flags; post(e)
            return
        }
        if modifierFlags[k] != nil {
            if !down, !mods.contains(k), let p = partner(k), mods.contains(p) { modifier(p, false); return }   // 进来时按左边补上的，松的却是右边
            modifier(k, down); return
        }
        if down { keys.insert(k) } else { keys.remove(k) }
        let e = CGEvent(keyboardEventSource: src, virtualKey: k, keyDown: down)
        e?.flags = flags
        post(e)
    }
    private func mouse(_ b: Int32, down: Bool) -> CGEvent? {
        let type: CGEventType, btn: CGMouseButton
        switch b {
        case 1: type = down ? .leftMouseDown : .leftMouseUp; btn = .left
        case 2: type = down ? .rightMouseDown : .rightMouseUp; btn = .right
        default: type = down ? .otherMouseDown : .otherMouseUp; btn = CGMouseButton(rawValue: UInt32(max(2, b - 1))) ?? .center
        }
        let e = CGEvent(mouseEventSource: src, mouseType: type, mouseCursorPosition: pos, mouseButton: btn)
        e?.flags = flags
        if b > 2 { e?.setIntegerValueField(.mouseEventButtonNumber, value: Int64(b - 1)) }
        return e
    }
    private func button(_ b: Int32, _ down: Bool, at p: CGPoint) {
        if p != pos { move(to: p) }
        let now = ProcessInfo.processInfo.systemUptime
        if down {
            let again = lastDown.button == b && now - lastDown.at < NSEvent.doubleClickInterval && hypot(p.x - lastDown.p.x, p.y - lastDown.p.y) < 5
            lastDown = (b, now, p, again ? lastDown.count + 1 : 1)
            buttons.insert(b)
        } else { buttons.remove(b) }
        let e = mouse(b, down: down)
        e?.setIntegerValueField(.mouseEventClickState, value: lastDown.button == b ? lastDown.count : 1)
        post(e)
    }
}

// MARK: 回调（C 函数指针，不能带上下文，用 ctx 找回 Share）

private let noticeCB: ywk_notice_fn = { ctx, json in
    guard let ctx, let json else { return }
    Unmanaged<Share>.fromOpaque(ctx).takeUnretainedValue().notice(String(cString: json))
}
private let inputCB: ywk_input_fn = { ctx, kind, a, b, c, d in
    guard let ctx else { return }
    Unmanaged<Share>.fromOpaque(ctx).takeUnretainedValue().injector.apply(kind, a, b, c, d)
}
private let tapCB: CGEventTapCallBack = { _, type, event, ctx in
    guard let ctx else { return Unmanaged.passUnretained(event) }
    return Unmanaged<Share>.fromOpaque(ctx).takeUnretainedValue().tapped(type, event)
}

// MARK: 键鼠共享

final class Share {
    static let shared = Share()

    struct Machine { var id: String; var name: String; var os: Int; var online = false }
    struct Found { var id: String; var name: String; var os: Int; var ip: String; var port: Int; var joining = false; var seen = ProcessInfo.processInfo.systemUptime }

    /// 界面要刷新（总在主线程调）
    var onChange: (() -> Void)?
    private(set) var running = false
    private(set) var problem: String?
    private(set) var store = ShareStore()
    private(set) var machines: [String: Machine] = [:]
    private(set) var found: [Found] = []
    private(set) var cells: [(id: String, x: Int, y: Int)] = []
    private(set) var myName = ""
    var me: String { store.id }
    var connected: Int { machines.values.filter(\.online).count }
    var demo = false                                   // 自检截图用：不连核心，界面照常画

    /// 正在发、正在收的文件（下拉里小图下面显示）
    struct Transfer { var key: String; var x: UInt32; var peer: String; var sending: Bool; var title: String; var done: Int64 = 0; var total: Int64 = 0; var state = 0; var err = "" }
    private(set) var transfers: [Transfer] = []
    private let notifier = Notifier()
    /// 小图上左键选中的那台（选中后按 ⌘V 把复制的文件发给它）
    var selected: String? { didSet { if selected != oldValue { slog(selected.map { "选中 \(machines[$0]?.name ?? $0)，下拉固定" } ?? "取消选中"); onPin?(selected != nil) } } }
    /// 选中了一台：下拉框固定住（点别处不收起），好把文件拖进来
    var onPin: ((Bool) -> Void)?
    var trusted: Bool { demo || AXIsProcessTrusted() }

    let injector = Injector()
    private var h: UnsafeMutableRawPointer?           // 核心。用它的都拿读锁（互相不挡，监听回调也不会被主线程堵住），只有开、关时拿写锁
    private let coreRW: UnsafeMutablePointer<pthread_rwlock_t> = { let p = UnsafeMutablePointer<pthread_rwlock_t>.allocate(capacity: 1); pthread_rwlock_init(p, nil); return p }()
    private let stateLock = NSLock()                   // 只护着下面几个小变量，拿着它的时候不做别的事
    private var remote = false                         // 外壳这边：本机指针是不是停在中间藏着（鼠标在别的电脑上）。监听回调问的是核心（ywk_remote）
    private var origin = CGPoint.zero
    private var size = CGSize.zero
    private var park = CGPoint.zero                    // 鼠标在别的电脑上时，本机指针停在这里
    private var tap: CFMachPort?
    private var tapGen = 0                             // 每次开监听加一：线程建好监听时发现已经不要了，就自己作废
    private var watchdog: Timer?
    private var secureHint = 0.0
    private var idleHidden = false                     // 对方的鼠标走了以后，本机指针藏着等本机鼠标动（拿 stateLock 碰）
    private var idleMoved = 0.0                        // 指针藏着的时候本机鼠标挪了多少（只在监听线程碰）
    private var mismatchText: String?
    private var stallText: String?
    private var tapLoop: CFRunLoop?
    private var tapping = false
    private var poll: Timer?
    private var sweep: Timer?
    private var screenWatch: Any?
    private var escapes: [TimeInterval] = []
    private var wheelRest = (x: 0.0, y: 0.0)
    /// 鼠标在别的电脑上时，刚把本机指针挪回停靠点：挪了多少、之后来过几个事件（只在监听线程碰）
    private var recenter: (w: CGPoint, seen: Int)?
    private var lastPark = 0.0                         // 上一次把本机指针挪回中间是什么时候（只在监听线程碰）
    /// 这次在对面：挪回停靠点几次、从移动量里减掉几次、没等到要减的几次（拿 stateLock 碰，回来时写进运行记录）
    private var parkStats = (warps: 0, fixed: 0, missed: 0)
    private var napGuard: NSObjectProtocol?
    // 剪贴板跟着指针走（见“剪贴板跟着指针走”一节；都只在主线程碰）
    private typealias ClipSnap = [[(NSPasteboard.PasteboardType, Data)]]
    private var ownClip: ClipSnap?                      // 客人进来之前本机自己的剪贴板；它走了放回去
    private var guestClips: [String: ClipSnap] = [:]    // 每台电脑带到本机来的那份：它的指针在本机上时才放进剪贴板
    private var nextGuestId: String?                    // 核心说“这台的指针要进来了”
    private var guestId: String?                        // 指针正在本机上的那台
    private var guestHasClip = false                    // 剪贴板里现在放的是客人的那份
    private var appliedClip = -1                        // 客人进来（或者它的那份放上来）之后剪贴板是第几版；走的时候变了 = 它在这儿复制过
    private var sentClip: [String: Int] = [:]           // 本机的指针上次带给这台电脑的是剪贴板的第几版
    private var remoteId: String?                       // 本机的指针正在哪台电脑上
    private var clipBoard = NSPasteboard.general        // 自检时换成一块临时的
    private var ownClipAt = -1                          // ownClip 是剪贴板第几版时存的：没变过就接着用
    private var tooBigAt = -1                           // 剪贴板第几版时提示过“文件太大没带过去”
    private var carryNo: UInt32 = 0                     // 本机往外带了几次（每带一次加一，附在带的内容上）
    /// 每台电脑、每个方向（键是“编号|是不是带回来的”）最近收到的是它第几次带的；那一次的文字留着，同一次的图片到了并成一份
    private var seenClip: [String: (no: UInt32, text: ClipSnap?)] = [:]
    /// 这一组现在的校验码（给面板显示）
    private(set) var groupCode = ""
    /// 等着用户点“允许”的加入申请
    private var asks: [(id: String, name: String)] = []
    private var asking = false
    private var askingID: String?

    private init() {}

    private func changed() { if Thread.isMainThread { onChange?() } else { DispatchQueue.main.async { self.onChange?() } } }
    private func withCore(_ f: (UnsafeMutableRawPointer) -> Void) { pthread_rwlock_rdlock(coreRW); if let h { f(h) }; pthread_rwlock_unlock(coreRW) }
    private var isRemote: Bool { stateLock.lock(); defer { stateLock.unlock() }; return remote }
    private func setIdle(_ v: Bool) { stateLock.lock(); idleHidden = v; stateLock.unlock() }
    /// 本机指针藏着等本机鼠标动的话，显示出来（主线程）
    private func showIdle() {
        stateLock.lock(); let was = idleHidden; idleHidden = false; let away = remote; stateLock.unlock()
        if was && !away { Cursor.show() }
    }
    private static func mainCenter() -> CGPoint { let b = CGDisplayBounds(CGMainDisplayID()); return CGPoint(x: b.midX, y: b.midY) }

    // MARK: 开关

    func start() {
        guard h == nil, !demo else { return }
        store = ShareStore.load()
        guard store.priv.count == 64 else { problem = "没能生成这台电脑的身份，请重新打开一苇"; changed(); return }
        // 从旧版升上来的：以前配对的记录用不上了，各台重新加入一次
        if store.group == nil { store.peers = []; store.cells = []; store.rev = 0 }
        machines = Dictionary(uniqueKeysWithValues: store.peers.map { ($0.id, Machine(id: $0.id, name: $0.name, os: $0.os)) })
        cells = store.cells.map { ($0.id, $0.x, $0.y) }
        myName = Host.current().localizedName ?? "Mac"
        let screen = Screens.union()
        origin = screen.origin; size = screen.size; park = Share.mainCenter(); injector.setOrigin(screen.origin)
        injector.onLeave = { [weak self] how in self?.guestLeft(how) }
        injector.onEnter = { [weak self] in self?.guestEntered() }
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        ywk_set_log(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Yiwei/share.log").path)
        slog("Mac 启动键鼠共享：辅助功能 \(AXIsProcessTrusted() ? "已授权" : "未授权")，屏幕 \(screen)")
        let handle = ywk_start(me, myName, 1, UInt32(screen.width), UInt32(screen.height), store.priv, store.pub, store.group ?? "", 0, 1, noticeCB, inputCB, ctx)
        guard let handle else {
            problem = "端口 47800 被占用"
            changed(); return
        }
        if let p = ywk_code(handle) { groupCode = String(cString: p); ywk_free(p) }
        ywk_set_take_back(handle, takeBack ? 1 : 0)
        pthread_rwlock_wrlock(coreRW); h = handle; pthread_rwlock_unlock(coreRW)
        problem = nil; running = true
        ywk_set_recv_dir(handle, Share.recvDir.path)
        // 一苇是没有窗口的后台程序，系统会让这类程序“打盹”（线程排得慢、定时不准）：别的电脑的鼠标经过它就会一顿一顿。
        // 开着键鼠共享时告诉系统这件事对延迟敏感（不拦着电脑睡眠）
        napGuard = ProcessInfo.processInfo.beginActivity(options: [.userInitiatedAllowingIdleSystemSleep, .latencyCritical], reason: "键鼠共享")
        notifier.attach()
        if !store.cells.isEmpty {
            ywk_restore_layout(handle, store.rev, store.cells.map { "\($0.id):\($0.x):\($0.y)" }.joined(separator: ","))
        }
        // 这台不急着放进摆法：两台各放各的会互相覆盖。第一次连上别的电脑时由编号小的那台把两台一起摆好（见 placeIfNeeded）
        screenWatch = NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in self?.screensChanged() }
        if trusted { startTap() } else { askPermission(); waitForPermission() }
        // 对方每 2 秒广播一次，核心半分钟才转告一次：70 秒没消息就当它不在了
        sweep = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            guard let self else { return }
            let now = ProcessInfo.processInfo.systemUptime, n = self.found.count
            self.found.removeAll { !$0.joining && now - $0.seen > 70 }
            if self.found.count != n { self.changed() }
        }
        changed()
    }

    func stop() {
        poll?.invalidate(); poll = nil
        sweep?.invalidate(); sweep = nil
        if let g = napGuard { ProcessInfo.processInfo.endActivity(g); napGuard = nil }
        watchdog?.invalidate(); watchdog = nil
        if let w = screenWatch { NotificationCenter.default.removeObserver(w); screenWatch = nil }
        // 先拿掉核心：等正在执行的监听回调做完，之后再不会有人把鼠标“穿走”
        pthread_rwlock_wrlock(coreRW); let old = h; h = nil; pthread_rwlock_unlock(coreRW)
        stopTap()
        forceLocal()
        if let old { ywk_stop(old) }
        forceLocal()                                             // 核心收尾时可能又说了一次“穿走 / 回来”：最后以本机为准
        injector.releaseAll()
        for k in machines.keys { machines[k]?.online = false }
        found.removeAll(); running = false
        clipReset()
        changed()
    }

    /// 不管之前是什么状态，本机鼠标一定放回来、显示出来
    private func forceLocal() {
        stateLock.lock(); remote = false; stateLock.unlock()
        CGAssociateMouseAndMouseCursorPosition(1)
        if Thread.isMainThread { Cursor.show() }
        DispatchQueue.main.async { Cursor.show() }              // 排在已经排队的“藏起来”后面
    }

    /// 鼠标在别的电脑上时，每半秒看一眼：监听坏了、权限被收回、本机开了安全输入，就把鼠标拉回本机，免得用户被困住
    private func startWatchdog() {
        watchdog?.invalidate()
        watchdog = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] t in
            guard let self else { t.invalidate(); return }
            guard self.isRemote else { t.invalidate(); self.watchdog = nil; return }
            self.stateLock.lock(); let tp = self.tap; self.stateLock.unlock()
            var broken = !AXIsProcessTrusted() || tp == nil
            if let tp, !CGEvent.tapIsEnabled(tap: tp) { CGEvent.tapEnable(tap: tp, enable: true); broken = broken || !CGEvent.tapIsEnabled(tap: tp) }
            let secure = IsSecureEventInputEnabled()
            guard broken || secure else { return }
            slog("看门狗：\(secure ? "本机开着安全输入" : "监听失效")，拉回本机")
            self.withCore { ywk_home($0) }                       // 核心随后会说“回来了”；不等它，先在本机恢复
            if self.isRemote { self.forceLocal() }
            if secure { self.problem = Share.secureText; self.secureHint = ProcessInfo.processInfo.systemUptime }
            t.invalidate(); self.watchdog = nil
            self.changed()
        }
    }
    static let secureText = "有程序开着安全输入，键盘带不过去"

    /// 第一次打开时请系统弹出“辅助功能”的授权提示
    func askPermission() {
        _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
    }
    func openPermissionSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") { NSWorkspace.shared.open(url) }
    }
    private func waitForPermission() {
        poll?.invalidate()
        poll = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] t in
            guard let self, self.running else { t.invalidate(); return }
            if AXIsProcessTrusted() { t.invalidate(); self.poll = nil; self.startTap(); self.changed() }
        }
    }

    private func screensChanged() {
        let r = Screens.union()
        stateLock.lock(); origin = r.origin; size = r.size; park = Share.mainCenter(); stateLock.unlock()
        injector.setOrigin(r.origin)
        withCore { ywk_set_size($0, UInt32(r.width), UInt32(r.height)) }
    }

    // MARK: 核心发来的事

    fileprivate func notice(_ json: String) {
        guard let d = json.data(using: .utf8), let o = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any], let t = o["t"] as? String else { return }
        switch t {
        case "grab": grab(o["id"] as? String ?? "")
        case "release": release(x: o["x"] as? Int ?? 0, y: o["y"] as? Int ?? 0)
        default: DispatchQueue.main.async { self.handle(t, o) }
        }
    }

    /// 鼠标穿到别的电脑上了：本机鼠标停住、藏起来，剪贴板带过去
    private func grab(_ id: String) {
        slog("鼠标穿走")
        // 本机指针要挪到主屏中间附近待着（停在边上会一直顶着自动隐藏的程序坞、触发角）。
        // 这里不挪：挪指针只在监听线程里做（见 route），因为系统会把“挪的那一段”算进挪完后下一个鼠标事件的移动量，
        // 得由看得到那个事件的线程把它减掉。2026-11-13 真机记录：在这里挪了没减，鼠标一穿过去就被这一段带回来，210 次没有一次留得住
        let c = Share.mainCenter()
        stateLock.lock(); remote = true; park = c; idleHidden = false; parkStats = (0, 0, 0); stateLock.unlock()
        DispatchQueue.main.async {
            guard self.isRemote else { return }                  // 排队的这会儿已经回来了（或者关掉了）
            if self.problem != nil && self.problem == self.stallText { self.problem = nil }
            self.remoteId = id
            Cursor.hide(); self.bringClip(to: id); self.startWatchdog(); self.changed()
        }
    }
    /// 鼠标回来了
    private func release(x: Int, y: Int) {
        stateLock.lock(); remote = false; let o = origin, ps = parkStats; stateLock.unlock()
        slog("鼠标回到本机 (\(x), \(y))；在对面期间把本机指针挪回中间 \(ps.warps) 次，从移动量里减掉 \(ps.fixed) 次，没等到要减的 \(ps.missed) 次")
        CGWarpMouseCursorPosition(CGPoint(x: o.x + CGFloat(x), y: o.y + CGFloat(y)))
        CGAssociateMouseAndMouseCursorPosition(1)
        DispatchQueue.main.async { if !self.isRemote { self.remoteId = nil }; Cursor.show(); self.changed() }
    }

    private func handle(_ t: String, _ o: [String: Any]) {
        let id = o["id"] as? String ?? "", name = o["name"] as? String ?? "", os = o["os"] as? Int ?? 0
        switch t {
        case "found":
            guard machines[id]?.online != true, let ip = o["ip"] as? String, let port = o["port"] as? Int else { return }
            if let i = found.firstIndex(where: { $0.id == id }) { found[i].ip = ip; found[i].port = port; found[i].name = name; found[i].seen = ProcessInfo.processInfo.systemUptime }
            else { found.append(Found(id: id, name: name, os: os, ip: ip, port: port)) }
        case "group":
            // 本机所在的组变了：存下来。加入了别人的组的话，原来组里的电脑和摆法都不要了
            groupCode = o["code"] as? String ?? ""
            store.group = o["save"] as? String
            if (o["joined"] as? Int ?? 0) == 1 { store.peers = []; store.cells = []; store.rev = 0; machines = [:]; cells = []; selected = nil }
            store.save()
        case "ask":
            guard askingID != id, !asks.contains(where: { $0.id == id }), asks.count < 3 else { return }   // 同一台别排两次，也别被人刷屏
            asks.append((id, name))
            askNext()
        case "join_failed":
            for i in found.indices { found[i].joining = false }
            let who = name.isEmpty ? "那台电脑" : "“\(name)”"
            let why = o["why"] as? Int ?? 4
            let a = NSAlert()
            a.messageText = why == 1 ? "校验码不对" : why == 2 ? "\(who)没有同意" : why == 3 ? "\(who)暂时不接受加入" : "没连上\(who)"
            a.informativeText = why == 1 ? "校验码在\(who)的面板里，点“展开”能看到。" : why == 3 ? "校验码连着输错了几次，过一分钟再试。" : why == 4 ? "看看它是不是开着键鼠共享、和这台在同一个网络里。" : ""
            DispatchQueue.main.async { NSApp.activate(ignoringOtherApps: true); a.runModal() }
        case "mismatch":
            // 两边的一苇不是同一版，连不上：告诉用户该更新哪一台
            let theirs = o["theirs"] as? Int ?? 0, mine = o["mine"] as? Int ?? 0
            mismatchText = theirs < mine ? "“\(name)”上的一苇是旧版，请更新它" : "“\(name)”上的一苇比这台新，请更新这台"
            problem = mismatchText
        case "stalled":
            // 发过去的键鼠对方一直没确认，核心已经断开它重连，鼠标回本机了
            stallText = "“\(name)”没有响应（它的防火墙可能挡住了一苇）"
            problem = stallText
        case "connected":
            if problem == Share.secureText || (problem != nil && problem == mismatchText) { problem = nil }
            machines[id] = Machine(id: id, name: name, os: os, online: true)
            found.removeAll { $0.id == id }
            // 记下组里有这台（没开机时在位置图上是灰的）
            if let i = store.peers.firstIndex(where: { $0.id == id }) { if store.peers[i].name != name { store.peers[i].name = name; store.save() } }
            else { store.peers.append(.init(id: id, key: "", name: name, os: os)); store.save() }
            // 对方可能马上把它那边存的摆法发过来；等一下再看要不要替它找个位置
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.placeIfNeeded(id) }
        case "disconnected":
            machines[id]?.online = false
            if selected == id { selected = nil }
            found.removeAll { $0.id == id }
            asks.removeAll { $0.id == id }
            if askingID == id { NSApp.abortModal() }            // 对方不等了、断开了：问话的框自己关掉
            clipForget(id)
        case "guest":
            nextGuestId = id
        case "clip":
            guard let text = o["text"] as? String else { return }
            let back = (o["back"] as? Int ?? 0) == 1, no = (o["no"] as? NSNumber)?.uint32Value ?? 0
            if text.isEmpty { if !back { clipCleared(from: id, no: no) } }
            else { clipArrived(from: id, back: back, no: no, image: false, snap: [[(.string, Data(text.utf8))]]) }
        case "clip_image":
            // 对方带来的图片（核心已经存成文件）
            guard let path = o["path"] as? String else { return }
            defer { try? FileManager.default.removeItem(atPath: path) }
            guard let d = FileManager.default.contents(atPath: path), let rep = NSBitmapImageRep(data: d) else { return }
            var piece: [(NSPasteboard.PasteboardType, Data)] = []
            if let png = rep.representation(using: .png, properties: [:]) { piece.append((.png, png)) }
            if let tiff = rep.tiffRepresentation { piece.append((.tiff, tiff)) }
            if !piece.isEmpty { clipArrived(from: id, back: (o["back"] as? Int ?? 0) == 1, no: (o["no"] as? NSNumber)?.uint32Value ?? 0, image: true, snap: [piece]) }
        case "clip_failed":
            sentClip[id] = nil                                  // 图片、文件半路没带成：下次指针过去重新带
        case "clip_files":
            // 对方带来的复制的文件、文件夹（核心已经收在临时文件夹里）：当成“复制了这几个文件”
            guard let paths = o["paths"] as? [String], !paths.isEmpty else { return }
            clipArrived(from: id, back: (o["back"] as? Int ?? 0) == 1, no: (o["no"] as? NSNumber)?.uint32Value ?? 0, image: false, snap: paths.map { Share.filePiece($0) })
        case "send_progress", "recv_progress":
            let key = (t == "send_progress" ? "s" : "r\(id)") + "\((o["x"] as? NSNumber)?.uint32Value ?? 0)"
            guard let i = transfers.firstIndex(where: { $0.key == key }) else { return }
            transfers[i].done = (o["done"] as? NSNumber)?.int64Value ?? 0; transfers[i].total = (o["total"] as? NSNumber)?.int64Value ?? 0
        case "recv_begin":
            let x = (o["x"] as? NSNumber)?.uint32Value ?? 0, count = o["count"] as? Int ?? 0
            let who = machines[id]?.name ?? "另一台电脑", what = (o["name"] as? String ?? "") + (count > 1 ? " 等 \(count) 个" : "")
            transfers.append(Transfer(key: "r\(id)\(x)", x: x, peer: id, sending: false, title: "收\(who)：\(what)", total: (o["total"] as? NSNumber)?.int64Value ?? 0))
        case "send_done", "recv_done":
            let key = (t == "send_done" ? "s" : "r\(id)") + "\((o["x"] as? NSNumber)?.uint32Value ?? 0)"
            let ok = o["ok"] as? Bool ?? false
            if let i = transfers.firstIndex(where: { $0.key == key }) {
                transfers[i].state = ok ? 1 : 2; transfers[i].err = o["err"] as? String ?? ""
                if ok { transfers[i].done = transfers[i].total }
                DispatchQueue.main.asyncAfter(deadline: .now() + (ok ? 3 : 6)) { self.transfers.removeAll { $0.key == key }; self.changed() }
            }
            if t == "recv_done", ok, let first = o["first"] as? String {
                let count = o["count"] as? Int ?? 1, total = (o["total"] as? NSNumber)?.int64Value ?? 0
                let name = (first as NSString).lastPathComponent
                notifier.post(title: "收到“\(machines[id]?.name ?? "另一台电脑")”发来的 \(count) 个文件",
                              body: "\(name)\(count > 1 ? " 等" : "")，共 \(ByteCountFormatter.string(fromByteCount: total, countStyle: .file))，存到 \(Share.recvDir.lastPathComponent)", path: first)
            }
        case "layout":
            guard let rev = o["rev"] as? NSNumber, let list = o["cells"] as? [[Any]] else { return }
            cells = list.compactMap { c in
                guard c.count == 3, let i = c[0] as? String, let x = c[1] as? Int, let y = c[2] as? Int else { return nil }
                return (i, x, y)
            }
            store.rev = rev.uint64Value
            store.cells = cells.map { .init(id: $0.id, x: $0.x, y: $0.y) }
            store.save()
        default: return
        }
        changed()
    }

    /// 新连上一台电脑后，两台里谁还没有位置就给谁找一个（编号小的那台来做，免得两边各放各的、互相覆盖）：
    /// 两台都没有：这台放 (0,0)、对方放右边；只缺一台：放到另一台旁边的空格
    private func placeIfNeeded(_ id: String) {
        guard running, machines[id]?.online == true, me < id else { return }
        var taken = cells.map { ($0.x, $0.y) }
        func spot(near a: (Int, Int)) -> (Int, Int) {
            let p = [(1, 0), (-1, 0), (0, 1), (0, -1)].map { (a.0 + $0.0, a.1 + $0.1) }.first { p in !taken.contains { $0 == p } }
                ?? (taken.map(\.0).max()! + 1, a.1)
            taken.append(p); return p
        }
        let mine = cells.first { $0.id == me }.map { ($0.x, $0.y) }, theirs = cells.first { $0.id == id }.map { ($0.x, $0.y) }
        var moves: [(String, (Int, Int))] = []
        switch (mine, theirs) {
        case (nil, nil): taken.append((0, 0)); moves = [(me, (0, 0)), (id, spot(near: (0, 0)))]
        case (nil, let t?): moves = [(me, spot(near: t))]
        case (let m?, nil): moves = [(id, spot(near: m))]
        default: return
        }
        for (who, p) in moves { withCore { ywk_place($0, who, Int32(p.0), Int32(p.1)) } }
    }

    private func askNext() {
        guard !asking, !asks.isEmpty else { return }
        asking = true
        let c = asks.removeFirst()
        askingID = c.id
        let a = NSAlert()
        a.messageText = "“\(c.name)”想加入这一组"
        a.informativeText = "它输对了校验码。允许后，它和组里每台电脑都能互相控制、互发文件。"
        a.addButton(withTitle: "允许"); a.addButton(withTitle: "不允许")
        NSApp.activate(ignoringOtherApps: true)
        let answer = a.runModal()
        askingID = nil
        if answer != .abort { withCore { ywk_confirm($0, c.id, answer == .alertFirstButtonReturn ? 1 : 0) } }
        asking = false
        changed()
        askNext()
    }

    // MARK: 界面上的操作

    /// 加入发现的一台电脑所在的组。code 是用户输入的、它的校验码
    func join(_ id: String, code: String) {
        guard let i = found.firstIndex(where: { $0.id == id }) else { return }
        found[i].joining = true
        let f = found[i]
        withCore { ywk_join($0, f.ip, UInt16(clamping: f.port), code) }
        changed()
        DispatchQueue.main.asyncAfter(deadline: .now() + 125) {                 // 一直没下文就让用户能再点（对方最多有两分钟点“允许”）
            if let j = self.found.firstIndex(where: { $0.id == id }), self.found[j].joining { self.found[j].joining = false; self.changed() }
        }
    }
    /// 手动填地址：“192.168.1.20”或者“192.168.1.20:47800”
    @discardableResult func connect(address: String, code: String) -> Bool {
        let parts = address.trimmingCharacters(in: .whitespaces).split(separator: ":").map(String.init)
        var a = in_addr()
        guard let ip = parts.first, ip.split(separator: ".").count == 4, inet_pton(AF_INET, ip, &a) == 1 else { return false }
        let port = parts.count > 1 ? UInt16(parts[1]) ?? 47800 : 47800
        withCore { ywk_join($0, ip, port, code) }
        return true
    }
    /// 这一组换一个校验码
    func newCode() { withCore { ywk_new_code($0) } }
    func place(_ id: String, _ x: Int, _ y: Int) { withCore { ywk_place($0, id, Int32(x), Int32(y)) } }
    /// 收到的文件放这里（配置文件里的 receive_dir）
    static var recvDir: URL { URL(fileURLWithPath: NSString(string: Config.load().receiveDir).expandingTildeInPath) }
    func setRecvDir(_ url: URL) {
        var c = Config.load(); c.receiveDir = (url.path as NSString).abbreviatingWithTildeInPath; c.save()
        withCore { ywk_set_recv_dir($0, url.path) }
        changed()
    }
    /// 发文件、文件夹给一台连着的电脑
    func send(_ urls: [URL], to id: String) {
        slog("要发 \(urls.count) 项给 \(machines[id]?.name ?? id)")
        guard !urls.isEmpty else { return }
        var x: UInt32 = 0
        withCore { x = ywk_send_files($0, id, urls.map(\.path).joined(separator: "\n")) }
        let who = machines[id]?.name ?? "另一台电脑", what = urls[0].lastPathComponent + (urls.count > 1 ? " 等 \(urls.count) 个" : "")
        var t = Transfer(key: "s\(x)", x: x, peer: id, sending: true, title: "发给\(who)：\(what)")
        if x == 0 { t.state = 2; t.err = "没连上"; t.key = "s0-\(UUID().uuidString)"; let k = t.key; DispatchQueue.main.asyncAfter(deadline: .now() + 4) { self.transfers.removeAll { $0.key == k }; self.changed() } }
        transfers.append(t)
        changed()
    }
    func cancel(_ key: String) {
        guard let t = transfers.first(where: { $0.key == key }), t.state == 0 else { return }
        withCore { t.sending ? ywk_cancel_send($0, t.x) : ywk_cancel_recv($0, t.peer, t.x) }
    }
    /// 右键自己的卡片：选收文件的文件夹
    func chooseRecvDir() {
        let p = NSOpenPanel()
        p.canChooseDirectories = true; p.canChooseFiles = false; p.canCreateDirectories = true; p.prompt = "选这里"
        p.message = "收到的文件放在哪里"
        p.directoryURL = Share.recvDir
        NSApp.activate(ignoringOtherApps: true)
        if p.runModal() == .OK, let u = p.url { setRecvDir(u) }
    }
    /// 右键别的电脑：选要发的文件、文件夹
    func chooseAndSend(to id: String) {
        let p = NSOpenPanel()
        p.canChooseDirectories = true; p.canChooseFiles = true; p.allowsMultipleSelection = true; p.prompt = "发送"
        p.message = "发给“\(machines[id]?.name ?? "另一台电脑")”"
        NSApp.activate(ignoringOtherApps: true)
        if p.runModal() == .OK { send(p.urls, to: id) }
    }
    func demoTransfer() { transfers = [Transfer(key: "s1", x: 1, peer: "", sending: true, title: "发给Windows 台式机：视频.mp4 等 3 个", done: 620_000_000, total: 1_000_000_000)] }

    /// 把一台电脑请出这一组：这一组随即换一个校验码，它拿着旧的进不来
    /// “鼠标被控切回”：开着时别的电脑的指针在这台上，这台自己的鼠标一动就请它回去
    var takeBack: Bool { store.takeBack ?? false }
    func setTakeBack(_ on: Bool) {
        if store.id.isEmpty { store = ShareStore.load() }
        store.takeBack = on; store.save()
        withCore { ywk_set_take_back($0, on ? 1 : 0) }
        changed()
    }
    func forget(_ id: String) {
        withCore { ywk_forget($0, id) }
        store.peers.removeAll { $0.id == id }; store.save()
        machines.removeValue(forKey: id)
        changed()
    }

    // MARK: 剪贴板跟着指针走
    // 甲的指针进到乙：乙的剪贴板临时换成甲的那份；甲的指针在乙上时复制的东西，指针走到哪带到哪；
    // 甲的指针离开乙：乙的剪贴板恢复成甲来之前的样子；甲在乙上什么都没复制：甲的剪贴板不变。
    // 规则和核心的 clipflow.rs（Windows 外壳用的那份）是同一套，改的时候两处一起改。这一节的函数都在主线程上跑。

    /// 把剪贴板整份存下来（所有格式）。超过 64MB 不存
    private func clipSnapshot() -> ClipSnap? {
        var total = 0
        var out: ClipSnap = []
        for item in clipBoard.pasteboardItems ?? [] {
            var one: [(NSPasteboard.PasteboardType, Data)] = []
            for t in item.types {
                guard let d = item.data(forType: t) else { continue }
                total += d.count
                if total > 64 << 20 { return nil }
                one.append((t, d))
            }
            if !one.isEmpty { out.append(one) }
        }
        return out
    }
    private static func sameClip(_ a: ClipSnap?, _ b: ClipSnap?) -> Bool {
        guard let a, let b else { return a == nil && b == nil }
        return a.count == b.count && zip(a, b).allSatisfy { x, y in x.count == y.count && zip(x, y).allSatisfy { $0.0 == $1.0 && $0.1 == $1.1 } }
    }
    /// 整份放进剪贴板，放成了返回 true。guest：这是别的电脑的指针带来的、临时用的，
    /// 加上剪贴板工具约定的“临时内容”记号（nspasteboard.org），不让它进剪贴板历史
    @discardableResult private func clipApply(_ s: ClipSnap, guest: Bool) -> Bool {
        clipBoard.clearContents()
        let items = s.filter { !$0.isEmpty }.map { one -> NSPasteboardItem in
            let i = NSPasteboardItem()
            for (t, d) in one { i.setData(d, forType: t) }
            return i
        }
        if guest, let first = items.first, !first.types.contains(Share.transientType) { first.setData(Data(), forType: Share.transientType) }
        return items.isEmpty || clipBoard.writeObjects(items)
    }
    private static let transientType = NSPasteboard.PasteboardType("org.nspasteboard.TransientType")
    /// 一个文件（文件夹）当成剪贴板里的一项
    private static func filePiece(_ path: String) -> [(NSPasteboard.PasteboardType, Data)] { [(.fileURL, Data(URL(fileURLWithPath: path).absoluteString.utf8))] }
    /// 把剪贴板里现在能带走的带给 id，附上“是不是带回去的、本机第几次带”。什么都带不了返回 false。
    /// 复制的是文件、文件夹：合起来 100MB 以内就带（原件不动），太大的不带、提示改用“发送文件”。
    /// 不然带文字、图片（图片存成临时文件，核心发完会删）；两样都有（比如从表格里复制）就都带，对面粘贴到哪种程序里都对
    private func carry(to id: String, back: Bool) -> (any: Bool, no: UInt32) {
        carryNo += 1
        let pb = clipBoard, no = carryNo, b: Int32 = back ? 1 : 0
        if pb.availableType(from: [.fileURL]) != nil {
            let urls = (pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
            let paths = urls.map(\.path).filter { !$0.contains("\n") }
            var r: Int32 = -1
            if !paths.isEmpty { withCore { r = ywk_clip_files($0, id, paths.joined(separator: "\n"), b, no) } }
            if r == 0, tooBigAt != pb.changeCount {              // 同一次复制只提示一回
                tooBigAt = pb.changeCount
                notifier.post(title: "复制的文件超过 100MB（或多于 2000 个），没有带过去", body: "在一苇的小图上右键那台电脑，选“发送文件”。", path: "")
            }
            return (r == 1, no)
        }
        var text = pb.string(forType: .string)
        if let t = text, t.isEmpty || t.utf8.count > 4 << 20 { text = nil }
        let image = Share.clipImageFile(pb)
        if let t = text { withCore { ywk_clip($0, id, t, b, no) } }
        if let p = image { withCore { ywk_clip_image($0, id, p, b, no) } }
        return (text != nil || image != nil, no)
    }
    /// 本机的指针进到 id 这台电脑：把剪贴板带过去。和上次带给它的是同一版就不带（它那边留着上次那份）
    private func bringClip(to id: String) {
        guard sentClip[id] != clipBoard.changeCount else { return }
        let r = carry(to: id, back: false)
        if !r.any { withCore { ywk_clip($0, id, "", 0, r.no) } }       // 没有能带的：告诉它别再用我上次带去的那份
        sentClip[id] = clipBoard.changeCount
        slog("指针到了 \(id.prefix(6))：剪贴板\(r.any ? "带过去" : "里没有能带的")")
    }
    /// 别的电脑的指针进来了：记下本机自己的剪贴板，换上它的那份
    private func guestEntered() {
        showIdle()
        guestId = nextGuestId
        // 本机的那份没变过就接着用上次存的，不把所有格式再读一遍（读一遍会逼着复制的那个程序把内容现做出来，大的会卡）
        if ownClip == nil || ownClipAt != clipBoard.changeCount { ownClip = clipSnapshot(); ownClipAt = clipBoard.changeCount }
        guestHasClip = false
        appliedClip = clipBoard.changeCount
        if let g = guestId, let s = guestClips[g] { showGuest(g, s) }
        slog("\(guestId?.prefix(6) ?? "?") 的指针进来了：剪贴板\(guestHasClip ? "换成它的那份" : "先不动")\(ownClip == nil ? "（本机的那份太大，存不下）" : "")")
    }
    /// 把客人的那份放进剪贴板，并把放好之后的样子记成它的那份：离开时拿来比，一样就是它在这儿没复制过
    private func showGuest(_ id: String, _ snap: ClipSnap) {
        guard clipApply(snap, guest: true) else { return }
        guestClips[id] = clipSnapshot() ?? snap
        guestHasClip = true; appliedClip = clipBoard.changeCount
    }
    /// 放回本机自己的那份
    private func restoreOwn() {
        if let own = ownClip, clipApply(own, guest: false) { ownClipAt = clipBoard.changeCount }
    }
    /// 它的指针走了。how：0 正常走了、1 被请回去了、2 掉线了。
    /// 它在这儿复制过东西就带回给它；本机的剪贴板恢复成它来之前的样子
    private func guestLeft(_ how: Int32) {
        if let g = guestId {
            let changed = clipBoard.changeCount != appliedClip
            let now = changed ? clipSnapshot() : nil
            // 版本变了不一定是复制过（剪贴板工具改写也会变）：内容和原来不一样才算。读不出来（太大）就当没复制过
            let copied = now != nil && !Share.sameClip(now, guestHasClip ? guestClips[g] : ownClip)
            if copied {
                if how != 2 { _ = carry(to: g, back: true) }
                // 剪贴板里现在是它新复制的：记成它的那份（它下次进来接着用，不用重新带）
                guestClips[g] = how == 2 ? nil : now
            }
            let dirty = changed || guestHasClip
            if dirty { restoreOwn() }
            slog("\(g.prefix(6)) 的指针走了（\(how)）：\(copied ? "它在这儿复制过，" + (how == 2 ? "掉线了带不回去" : "带回给它") : "它在这儿没复制过")，本机的剪贴板\(dirty ? (ownClip == nil ? "恢复不了" : "恢复原样") : "没动过")")
        }
        guestHasClip = false; guestId = nil
        appliedClip = clipBoard.changeCount
        // 正常走的：本机指针先藏起来放到中间，本机鼠标一动再出现（只看得到一个指针）。
        // 轮到这里的时候它可能已经又进来了（来回穿得快，上面整理剪贴板又要一点时间）：那就不能再把指针挪到中间藏起来，
        // 不然它眼看着指针从边上跳到屏幕中间又跳回去
        guard how == 0, !isRemote, !injector.hosting else { return }
        CGWarpMouseCursorPosition(Share.mainCenter()); CGAssociateMouseAndMouseCursorPosition(1)
        Cursor.hide(); setIdle(true)
    }
    /// 别的电脑带来了一份剪贴板内容（arrived：一段文字、一张图片各是一项，几个文件是几项）。
    /// back：本机的指针在它上面时复制的，跟着指针回来了；不然是它的指针带到本机来的。no：它第几次往外带
    private func clipArrived(from id: String, back: Bool, no: UInt32, image: Bool, snap arrived: ClipSnap) {
        let key = "\(id)|\(back)", last = seenClip[key]
        if let last, no < last.no { return }                     // 比已经收到的还早的一次：晚到了，不要
        var snap = arrived
        // 同一次带的文字先到、图片后到：并成一份
        if image, let last, last.no == no, let first = last.text?.first, let piece = arrived.first { snap = [first + piece] }
        seenClip[key] = (no, image ? nil : snap)
        if back {
            // 这会儿本机上有客人的话，它算“本机自己的那份”：客人走了，剪贴板里就是它。客人没往剪贴板里放过东西就现在换上
            if guestId == nil { clipApply(snap, guest: false) }
            else {
                if !guestHasClip && clipBoard.changeCount == appliedClip && clipApply(snap, guest: false) { appliedClip = clipBoard.changeCount; ownClipAt = appliedClip }
                ownClip = snap
            }
            sentClip[id] = clipBoard.changeCount                 // 它那边留着这一份，下次过去不用再带
            if let r = remoteId, r != id { bringClip(to: r) }    // 指针已经到了另一台：接着带过去
        } else if id != guestId {
            guestClips[id] = snap                                // 它的指针不在本机上：先记着（带来的可能比“进来了”早到一步）
        } else if clipBoard.changeCount == appliedClip {
            showGuest(id, snap)
        }                                                        // 不然是它刚在这儿复制了新的：带来的那份更早，不拿它盖掉
    }
    /// id 说它这次（第 no 次带）没有能带的（比如它复制的是文件）：它上次带来的那份不要了
    private func clipCleared(from id: String, no: UInt32) {
        let key = "\(id)|false"
        if let last = seenClip[key], no < last.no { return }
        seenClip[key] = (no, nil)
        guestClips[id] = nil
        guard id == guestId, guestHasClip, clipBoard.changeCount == appliedClip else { return }
        restoreOwn()
        guestHasClip = false; appliedClip = clipBoard.changeCount
    }
    /// 一台电脑断开了：它重新连上以后，剪贴板从头带（它重开以后“第几次带”也从头数）
    private func clipForget(_ id: String) {
        sentClip[id] = nil; guestClips[id] = nil; seenClip["\(id)|true"] = nil; seenClip["\(id)|false"] = nil
    }
    /// 键鼠共享关掉了：客人还在、剪贴板被它动过，就恢复成本机自己的；其余都忘掉
    private func clipReset() {
        if guestId != nil && (guestHasClip || clipBoard.changeCount != appliedClip) { restoreOwn() }
        ownClip = nil; ownClipAt = -1; guestClips = [:]; nextGuestId = nil; guestId = nil; guestHasClip = false; sentClip = [:]; remoteId = nil; seenClip = [:]
    }
    /// 自检：不连别的电脑、不碰真的剪贴板（用一块临时的），把“带来、进来、复制、离开”走一遍
    static func clipSelfTest() -> Bool {
        let s = Share.shared, pb = NSPasteboard.withUniqueName()
        s.clipBoard = pb
        defer { s.clipReset(); s.clipBoard = .general; pb.releaseGlobally() }
        var ok = true
        func check(_ what: String, _ cond: Bool) { print("\(cond ? "通过" : "没通过") 剪贴板跟着指针走：\(what)"); if !cond { ok = false } }
        func set(_ t: String) { pb.clearContents(); pb.setString(t, forType: .string) }
        func text(_ t: String) -> [(NSPasteboard.PasteboardType, Data)] { [(.string, Data(t.utf8))] }
        func enter(_ id: String) { s.nextGuestId = id; s.guestEntered() }
        func came(_ id: String, _ no: UInt32, _ t: String) { s.clipArrived(from: id, back: false, no: no, image: false, snap: [text(t)]) }
        var now: String? { pb.string(forType: .string) }
        guard let png = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 2, pixelsHigh: 2, bitsPerSample: 8, samplesPerPixel: 3, hasAlpha: false, isPlanar: false,
                                         colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)?.representation(using: .png, properties: [:]) else { return false }
        // 本机当乙。离开都用“被请回去”（1）：和正常走只差挪不挪本机指针，自检不动用户的鼠标
        set("乙自己的")
        came("A", 1, "甲的")
        check("甲带来的比“进来了”早到一步：先记着，不动本机的", now == "乙自己的")
        enter("A")
        check("规则 1：甲的指针进来，换成甲的那份（带着“临时内容”记号）", now == "甲的" && pb.types?.contains(Share.transientType) == true)
        s.guestLeft(1)
        check("规则 3：甲的指针走了，恢复成原来的（不带记号）", now == "乙自己的" && pb.types?.contains(Share.transientType) != true)
        enter("A")
        check("甲没复制新的，再进来还是那份", now == "甲的")
        pb.clearContents(); pb.setString("甲的", forType: .string); pb.setData(Data(), forType: Share.transientType)
        s.guestLeft(1)
        check("规则 4：客人的那份在剪贴板上，版本变了、内容没变，不算它复制过", now == "乙自己的" && s.sentClip.isEmpty && Share.sameClip(s.guestClips["A"], [[(.string, Data("甲的".utf8)), (Share.transientType, Data())]]))
        enter("A")
        set("甲在乙上复制的")
        s.guestLeft(1)
        check("甲在乙上复制过，走了以后乙照样恢复原样", now == "乙自己的")
        enter("A")
        check("甲再进来：是它上次在乙上复制的", now == "甲在乙上复制的")
        came("A", 2, "甲新复制的")
        s.clipArrived(from: "A", back: false, no: 2, image: true, snap: [[(.png, png)]])
        check("同一次带来的文字和图片：两样都在", now == "甲新复制的" && pb.data(forType: .png) == png)
        s.clipArrived(from: "A", back: false, no: 3, image: true, snap: [[(.png, png)]])
        check("另一次带来的图片不和上次的文字并", now == nil && pb.data(forType: .png) == png)
        came("A", 2, "晚到的旧内容")
        check("更早那次带的晚到了：不要", now == nil)
        s.clipCleared(from: "A", no: 4)
        check("甲说这次没有能带的：换回本机的", now == "乙自己的")
        came("A", 5, "甲带来的字")
        set("甲刚在乙上复制的")
        s.clipArrived(from: "A", back: false, no: 5, image: true, snap: [[(.png, png)]])
        check("规则 2：甲刚在乙上复制了新的，晚到的图片不盖掉它", now == "甲刚在乙上复制的")
        s.guestLeft(1)
        enter("A"); s.clipCleared(from: "A", no: 6)
        s.clipArrived(from: "C", back: true, no: 1, image: false, snap: [text("在丙上复制的")])
        check("本机的指针在丙上复制的带回来了（甲还在）：算本机自己的，现在就能用", now == "在丙上复制的")
        came("A", 7, "甲又带来的")
        check("甲又带来一份：换成甲的", now == "甲又带来的")
        s.guestLeft(1)
        check("甲走了：剪贴板里是从丙带回来的那份", now == "在丙上复制的")
        enter("D"); s.guestLeft(1)
        check("没带东西的客人来了又走：本机的不变", now == "在丙上复制的" && s.guestClips["D"] == nil)
        enter("D"); set("在丙上复制的"); s.guestLeft(1)
        check("客人没带东西，版本变了、内容没变：不算它复制过", now == "在丙上复制的" && s.guestClips["D"] == nil)
        enter("D"); set("丁在本机上复制的"); s.guestLeft(1)
        check("客人在本机上复制过：记成它的那份，本机恢复原样", now == "在丙上复制的" && s.guestClips["D"] != nil)
        came("A", 8, "甲不在时带来的")
        check("客人不在时带来的：不动本机的", now == "在丙上复制的")
        enter("A")
        s.clipArrived(from: "A", back: false, no: 9, image: false, snap: [Share.filePiece("/tmp/一.txt"), Share.filePiece("/tmp/二")])
        let files = (pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        check("甲带来两个文件：剪贴板里就是“复制了这两个文件”", files.map(\.lastPathComponent) == ["一.txt", "二"])
        s.guestLeft(1)
        check("甲走了：恢复原样", now == "在丙上复制的" && pb.availableType(from: [.fileURL]) == nil)
        enter("D"); s.clipReset()
        check("关掉键鼠共享时客人的那份还在：恢复成本机自己的", now == "在丙上复制的")
        return ok
    }
    /// 把剪贴板里的图片存成一个临时的 BMP 文件，返回路径（核心发完会删）。用 BMP 是因为三个系统都认、不用带解码库；
    /// 不带透明（Windows 的剪贴板图片不带），透明的地方垫白
    private static func clipImageFile(_ pb: NSPasteboard) -> String? {
        guard let d = pb.data(forType: .png) ?? pb.data(forType: .tiff), let src = NSBitmapImageRep(data: d) else { return nil }
        let w = src.pixelsWide, h = src.pixelsHigh
        guard w > 0, h > 0, w * h * 3 <= 60 << 20 else { return nil }
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8, samplesPerPixel: 3, hasAlpha: false, isPlanar: false,
                                         colorSpaceName: .deviceRGB, bytesPerRow: w * 4, bitsPerPixel: 32),
              let ctx = NSGraphicsContext(bitmapImageRep: rep) else { return nil }
        NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = ctx
        NSColor.white.setFill(); NSRect(x: 0, y: 0, width: w, height: h).fill()
        src.draw(in: NSRect(x: 0, y: 0, width: w, height: h))
        NSGraphicsContext.restoreGraphicsState()
        guard var bmp = rep.representation(using: .bmp, properties: [:]), bmp.count > 54 else { return nil }
        // 系统写出来的是“从上往下”的（高度是负数）。Windows 上不少程序只认“从下往上”的：把各行倒过来，高度改成正数
        let height = bmp.subdata(in: 22..<26).withUnsafeBytes { $0.load(as: Int32.self) }
        let start = Int(bmp.subdata(in: 10..<14).withUnsafeBytes { $0.load(as: UInt32.self) }), row = (w * 24 + 31) / 32 * 4
        if height < 0, start + row * h <= bmp.count {
            var flipped = bmp.prefix(start)
            for y in stride(from: h - 1, through: 0, by: -1) { flipped.append(bmp.subdata(in: start + y * row ..< start + (y + 1) * row)) }
            withUnsafeBytes(of: Int32(h).littleEndian) { flipped.replaceSubrange(22..<26, with: $0) }
            bmp = flipped
        }
        let path = NSTemporaryDirectory() + "yiwei-clip-\(UUID().uuidString).bmp"
        return FileManager.default.createFile(atPath: path, contents: bmp) ? path : nil
    }

    /// 这台电脑在局域网里的地址（给“手动添加”用）
    var address: String? {
        var ifs: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&ifs) == 0 else { return nil }
        defer { freeifaddrs(ifs) }
        var best: String?
        var p = ifs
        while let i = p?.pointee {
            defer { p = i.ifa_next }
            guard let sa = i.ifa_addr, sa.pointee.sa_family == UInt8(AF_INET), i.ifa_flags & UInt32(IFF_UP) != 0, i.ifa_flags & UInt32(IFF_LOOPBACK) == 0 else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            guard getnameinfo(sa, socklen_t(sa.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 else { continue }
            let ip = String(cString: host), name = String(cString: i.ifa_name)
            if ip.hasPrefix("169.254.") { continue }
            if name.hasPrefix("en") { return ip }
            if best == nil { best = ip }
        }
        return best
    }

    // MARK: 监听本机键鼠（单独一个线程，不受主线程忙不忙影响）

    private func startTap() {
        guard !tapping, h != nil else { return }
        tapping = true
        stateLock.lock(); tapGen += 1; let gen = tapGen; stateLock.unlock()
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        let types: [CGEventType] = [.mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged, .leftMouseDown, .leftMouseUp,
                                    .rightMouseDown, .rightMouseUp, .otherMouseDown, .otherMouseUp, .scrollWheel, .keyDown, .keyUp, .flagsChanged]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << CGEventMask($1.rawValue)) }
        let t = Thread { [weak self] in
            guard let self else { return }
            guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap, eventsOfInterest: mask, callback: tapCB, userInfo: ctx) else {
                slog("键鼠监听开不了")
                DispatchQueue.main.async { self.tapping = false; self.problem = "没能监听键鼠，请检查辅助功能权限"; self.changed() }
                return
            }
            let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
            let loop = CFRunLoopGetCurrent()
            CFRunLoopAddSource(loop, source, .commonModes)
            self.stateLock.lock()
            guard gen == self.tapGen else { self.stateLock.unlock(); CFMachPortInvalidate(tap); return }   // 建好时已经被关掉了
            self.tap = tap; self.tapLoop = loop
            self.stateLock.unlock()
            CGEvent.tapEnable(tap: tap, enable: true)
            slog("键鼠监听已开始")
            CFRunLoopRun()
        }
        t.name = "一苇·键鼠监听"; t.qualityOfService = .userInteractive
        t.start()
    }
    private func stopTap() {
        stateLock.lock(); let (t, l) = (tap, tapLoop); tap = nil; tapLoop = nil; tapGen += 1; stateLock.unlock()
        if let t { CGEvent.tapEnable(tap: t, enable: false); CFMachPortInvalidate(t) }
        if let l { CFRunLoopStop(l) }
        tapping = false
    }

    fileprivate func tapped(_ type: CGEventType, _ e: CGEvent) -> Unmanaged<CGEvent>? {
        let pass = Unmanaged.passUnretained(e)
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            slog("键鼠监听被系统暂停（\(type == .tapDisabledByTimeout ? "超时" : "用户输入")），重新打开")
            stateLock.lock(); let t = tap; stateLock.unlock()
            if let t { CGEvent.tapEnable(tap: t, enable: true) }
            return pass
        }
        if e.getIntegerValueField(.eventSourceUserData) == injectedMark { return pass }
        pthread_rwlock_rdlock(coreRW); defer { pthread_rwlock_unlock(coreRW) }
        guard let h else { return pass }
        return route(h, type, e) ? nil : pass
    }

    /// 报给核心；返回 true 表示这次输入拿走了（给了别的电脑），本机不要再处理。
    /// 这里是系统的监听回调：只读几个状态、调核心那几个“放进队列就返回”的函数。穿不穿屏由核心的后台线程判断
    private func route(_ h: UnsafeMutableRawPointer, _ type: CGEventType, _ e: CGEvent) -> Bool {
        stateLock.lock(); let o = origin, sz = size, c = park, hid = idleHidden; stateLock.unlock()
        let remote = ywk_remote(h) == 1
        // 对方的鼠标走了以后本机指针藏着：本机键鼠真的动了（不是轻轻碰一下）就显示出来
        if hid && !remote {
            let real: Bool
            switch type {
            case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
                idleMoved += abs(e.getDoubleValueField(.mouseEventDeltaX)) + abs(e.getDoubleValueField(.mouseEventDeltaY))
                real = idleMoved >= 6
            case .keyUp, .leftMouseUp, .rightMouseUp, .otherMouseUp: real = false
            default: real = true
            }
            if real { idleMoved = 0; DispatchQueue.main.async { self.showIdle() } }
        } else { idleMoved = 0 }
        switch type {
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            let dx = e.getDoubleValueField(.mouseEventDeltaX), dy = e.getDoubleValueField(.mouseEventDeltaY)
            if remote {
                // 系统会把“程序挪指针的那一段”算进挪完后下一个真实鼠标事件的移动量里：认出来减掉。
                // 认法：减掉那一段以后明显变小的就是它（那一段有几百像素，手一下动不了这么多）。等了 8 个事件还没来就不等了
                var mx = dx, my = dy
                if let r = recenter {
                    if abs(mx - r.w.x) + abs(my - r.w.y) < abs(mx) + abs(my) {
                        mx -= r.w.x; my -= r.w.y; recenter = nil
                        stateLock.lock(); parkStats.fixed += 1; stateLock.unlock()
                    } else if r.seen >= 8 {
                        recenter = nil
                        stateLock.lock(); parkStats.missed += 1; stateLock.unlock()
                    } else { recenter = (r.w, r.seen + 1) }
                }
                ywk_remote_move(h, mx, my)
                let p = e.location
                ywk_trace(1, dx, dy, p.x - c.x, p.y - c.y, (Double(clock_gettime_nsec_np(CLOCK_UPTIME_RAW)) - Double(e.timestamp)) / 1e6, recenter == nil ? 0 : 1)
                // 只在两种时候把藏着的本机指针挪回主屏中间：刚穿走（它还顶在边上）；之后又碰到了屏幕边（免得顶出程序坞、触发角）。
                // 半秒最多一次。2026-11-13 真机记录：之前离中间远了就挪，12 秒挪了 432 次，除第一次外都没起作用，白白每秒打扰系统 35 次
                let now = ProcessInfo.processInfo.systemUptime
                let edge = p.x <= o.x + 2 || p.y <= o.y + 2 || p.x >= o.x + sz.width - 3 || p.y >= o.y + sz.height - 3
                if recenter == nil, edge, now - lastPark > 0.5 {
                    // 挪完紧接着“接上指针”：不然系统会把真实鼠标压住约 0.25 秒
                    CGWarpMouseCursorPosition(c); CGAssociateMouseAndMouseCursorPosition(1)
                    recenter = (CGPoint(x: c.x - p.x, y: c.y - p.y), 0); lastPark = now
                    stateLock.lock(); parkStats.warps += 1; stateLock.unlock()
                }
                return true
            }
            recenter = nil
            let p = e.location, x = p.x - o.x, y = p.y - o.y
            // 贴着边、本机又开着“安全输入”时不穿：穿过去打的字其实会打进本机那个程序
            if (x <= 0 || y <= 0 || x >= sz.width - 1 || y >= sz.height - 1) && IsSecureEventInputEnabled() {
                let now = ProcessInfo.processInfo.systemUptime
                if now - secureHint > 5 { secureHint = now; DispatchQueue.main.async { self.problem = Share.secureText; self.changed() } }
                return false
            }
            // 贴边了，可能要穿屏：先按系统说的对一下修饰键（抬起没监听到的补上，免得带着它穿过去）
            if x <= 0 || y <= 0 || x >= sz.width - 1 || y >= sz.height - 1 { ywk_sync_mods(h, heldMods(e.flags.rawValue) | (e.flags.contains(.maskAlphaShift) ? 0x100 : 0)) }
            // 报给核心的移动量每个方向最多算 1.5：系统给的移动量会混进“别的电脑把指针挪了多远”（真机记录里一下 -446,307），
            // 核心只看方向（贴着边时是不是还在往外推），这样够了。x、y 照实报：别的电脑的鼠标在本机上时，核心让它接着从这里走
            ywk_local_move(h, x, y, max(-1.5, min(1.5, dx)), max(-1.5, min(1.5, dy)))
            return false
        case .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .otherMouseDown, .otherMouseUp:
            let down = type == .leftMouseDown || type == .rightMouseDown || type == .otherMouseDown
            let b: Int32 = (type == .leftMouseDown || type == .leftMouseUp) ? 1 : (type == .rightMouseDown || type == .rightMouseUp) ? 2 : Int32(e.getIntegerValueField(.mouseEventButtonNumber)) + 1
            return ywk_button(h, b, down ? 1 : 0) == 1
        case .scrollWheel:
            // 鼠标在本机：滚轮给本机，核心用不着（报一声只是保持接口一致）。手离开以后还在滑的那一段（惯性滚动）不报
            guard remote else { if e.getIntegerValueField(.scrollWheelEventMomentumPhase) == 0 { _ = ywk_wheel(h, 0, 0) }; return false }
            wheelRest.y += e.getDoubleValueField(.scrollWheelEventPointDeltaAxis1) * 12
            wheelRest.x -= e.getDoubleValueField(.scrollWheelEventPointDeltaAxis2) * 12
            let ix = Int32(wheelRest.x.rounded(.towardZero)), iy = Int32(wheelRest.y.rounded(.towardZero))
            wheelRest.x -= Double(ix); wheelRest.y -= Double(iy)
            if ix != 0 || iy != 0 { _ = ywk_wheel(h, ix, iy) }
            return true
        case .keyDown, .keyUp:
            let code = Int32(e.getIntegerValueField(.keyboardEventKeycode)), down = type == .keyDown
            if down && code == 53 && remote && e.getIntegerValueField(.keyboardEventAutorepeat) == 0 {
                // 紧急拉回：连按三下 Esc
                let now = ProcessInfo.processInfo.systemUptime
                escapes = escapes.filter { now - $0 < 0.8 } + [now]
                if escapes.count >= 3 { escapes.removeAll(); ywk_home(h); return true }
            }
            let usage = ywk_usage_from_mac(code)
            guard usage >= 0 else { return remote }
            return ywk_key(h, usage, down ? 1 : 0) == 1
        case .flagsChanged:
            let k = CGKeyCode(e.getIntegerValueField(.keyboardEventKeycode))
            let usage = ywk_usage_from_mac(Int32(k))
            guard usage >= 0 else { return remote }
            if k == capsLock {                                          // 大写锁定每按一下只来一次：当成按下再抬起
                let taken = ywk_key(h, usage, 1) == 1
                _ = ywk_key(h, usage, 0)
                return taken
            }
            guard let bits = modifierFlags[k] else { return remote }
            let pairBits = bits.1 | (partner(k).flatMap { modifierFlags[$0]?.1 } ?? 0)
            // 有的虚拟键盘发来的事件不带左右分开的标志位：那就看通用的
            let down = e.flags.rawValue & pairBits == 0 ? e.flags.rawValue & bits.0 != 0 : e.flags.rawValue & bits.1 != 0
            return ywk_key(h, usage, down ? 1 : 0) == 1
        default:
            return false
        }
    }

    // MARK: 自检截图用的样子

    func showDemo(found withFound: Bool) {
        demo = true; running = true; problem = nil
        groupCode = "K7M4P2XQ"
        store.id = "11111111111111111111111111111111"
        myName = "Mac mini"
        machines = ["22222222222222222222222222222222": Machine(id: "22222222222222222222222222222222", name: "Windows 台式机", os: 2, online: true),
                    "33333333333333333333333333333333": Machine(id: "33333333333333333333333333333333", name: "Linux 笔记本", os: 3, online: false)]
        cells = [("22222222222222222222222222222222", -1, 0), ("11111111111111111111111111111111", 0, 0), ("33333333333333333333333333333333", 2, 1)]
        found = withFound ? [Found(id: "44444444444444444444444444444444", name: "书房的 ThinkPad", os: 3, ip: "192.168.1.30", port: 47800)] : []
    }
}

/// 下拉里一行发送 / 接收：标题、百分比（或结果）、取消、进度条
final class JobRow: NSStackView {
    let key: String
    private let title = NSTextField(labelWithString: ""), right = NSTextField(labelWithString: ""), x = NSButton(title: "✕", target: nil, action: nil), bar = NSProgressIndicator()
    init(key: String, target: AnyObject, action: Selector) {
        self.key = key
        super.init(frame: .zero)
        orientation = .vertical; alignment = .leading; spacing = 2
        title.font = .systemFont(ofSize: 12); title.lineBreakMode = .byTruncatingTail; title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        right.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        x.target = target; x.action = action; x.isBordered = false; x.identifier = NSUserInterfaceItemIdentifier(key); x.font = .systemFont(ofSize: 11); x.contentTintColor = .secondaryLabelColor; x.toolTip = "取消"
        bar.isIndeterminate = false; bar.minValue = 0; bar.maxValue = 1; bar.controlSize = .small; bar.style = .bar
        let line = NSStackView(views: [title, right, x]); line.orientation = .horizontal; line.spacing = 6
        addArrangedSubview(line); addArrangedSubview(bar)
        line.widthAnchor.constraint(equalTo: widthAnchor).isActive = true
        bar.widthAnchor.constraint(equalTo: widthAnchor).isActive = true
    }
    required init?(coder: NSCoder) { fatalError() }
    func show(_ t: Share.Transfer) {
        let pct = t.total > 0 ? Double(t.done) / Double(t.total) : 0
        title.stringValue = t.state == 1 ? (t.sending ? "已" + t.title : t.title.replacingOccurrences(of: "收", with: "已收", options: .anchored)) : t.title
        right.stringValue = t.state == 2 ? t.err : (t.state == 1 ? "完成" : "\(Int(pct * 100))%")
        right.textColor = t.state == 2 ? Palette.red : .secondaryLabelColor
        x.isHidden = t.state != 0; bar.isHidden = t.state != 0; bar.doubleValue = pct
    }
}

/// 下拉框整块都能接住拖进来的文件：选中了一台电脑时，发给它
final class DropStack: NSStackView {
    override init(frame: NSRect) { super.init(frame: frame); registerForDraggedTypes([.fileURL]) }
    required init?(coder: NSCoder) { fatalError() }
    override func draggingEntered(_ s: NSDraggingInfo) -> NSDragOperation { Share.shared.selected == nil ? [] : .copy }
    override func draggingUpdated(_ s: NSDraggingInfo) -> NSDragOperation { Share.shared.selected == nil ? [] : .copy }
    override func performDragOperation(_ s: NSDraggingInfo) -> Bool {
        slog("文件拖进下拉框（选中：\(Share.shared.selected ?? "无")）")
        guard let id = Share.shared.selected else { return false }
        let urls = (s.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        Share.shared.send(urls, to: id)
        return !urls.isEmpty
    }
}

// MARK: 收到文件的系统通知：点一下在访达里打开

final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    private var asked = false
    /// 一启动就接上：重启前留下的通知点了也能打开
    func attach() { UNUserNotificationCenter.current().delegate = self }
    func post(title: String, body: String, path: String) {
        let c = UNUserNotificationCenter.current()
        c.delegate = self
        let send = {
            let n = UNMutableNotificationContent(); n.title = title; n.body = body; n.userInfo = ["path": path]; n.sound = .default
            c.add(UNNotificationRequest(identifier: UUID().uuidString, content: n, trigger: nil))
        }
        if asked { send(); return }
        c.requestAuthorization(options: [.alert, .sound]) { _, _ in DispatchQueue.main.async { self.asked = true; send() } }
    }
    func userNotificationCenter(_ c: UNUserNotificationCenter, willPresent n: UNNotification, withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) { done([.banner, .sound]) }
    func userNotificationCenter(_ c: UNUserNotificationCenter, didReceive r: UNNotificationResponse, withCompletionHandler done: @escaping () -> Void) {
        if let p = r.notification.request.content.userInfo["path"] as? String, !p.isEmpty { DispatchQueue.main.async { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: p)]) } }
        done()
    }
}

// MARK: 位置图（下拉里的小图，以及展开后可以拖的大图）

private enum Palette {
    static let gold = NSColor(srgbRed: 0.949, green: 0.718, blue: 0.020, alpha: 1)
    static let red = NSColor(srgbRed: 0.761, green: 0.290, blue: 0.196, alpha: 1)
    static let card = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(white: 0.26, alpha: 1) : .white }
    static let border = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(white: 0.42, alpha: 1) : NSColor(srgbRed: 0.83, green: 0.83, blue: 0.82, alpha: 1) }
    static let well = NSColor(name: nil) { $0.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(white: 1, alpha: 0.06) : NSColor(white: 0, alpha: 0.04) }
}

/// 三个系统的小图形（不用官方标志）：苹果、旗子、企鹅。路径是 24×24 的格子里画的
private enum Glyph {
    static let paths: [Int: [(String, Bool, CGFloat)]] = [      // (路径, 是否填充, 线宽)
        1: [("M12 8.2c-1.6-1.3-3.6-1.5-5.2-.5C4.9 8.9 4.2 11.2 4.7 13.8c.6 3 2.4 5.9 4.4 6.3 1.1.2 1.9-.5 2.9-.5s1.8.7 2.9.5c2-.4 3.8-3.3 4.4-6.3.5-2.6-.2-4.9-2.1-6.1-1.6-1-3.6-.8-5.2.5z", false, 1.6),
            ("M12 8.2c0-1.7.5-3 1.5-4", false, 1.6), ("M13.4 5.6c1.1-1.4 2.6-1.9 4-1.6-.2 1.6-1.5 2.8-3.4 2.7", false, 1.6)],
        2: [("M3.5 7c2.8-1.6 5.4-1.1 8.2 0s5.4 1.5 8.8 0v10c-3.4 1.5-6 1.1-8.8 0s-5.4-1.6-8.2 0z", false, 1.6)],
        3: [("M12 3.4c-3 0-4.5 2.4-4.5 5.3 0 1.7-.9 2.7-1.7 4.5-1 2.4 0 5.5 2 6.9h8.4c2-1.4 3-4.5 2-6.9-.8-1.8-1.7-2.8-1.7-4.5 0-2.9-1.5-5.3-4.5-5.3z", false, 1.6),
            ("M12 10.8c-1.9 0-2.9 1.9-2.9 4.3s1.1 4.4 2.9 4.4 2.9-2 2.9-4.4-1-4.3-2.9-4.3z", false, 1.6),
            ("M10.3 7.6h.01M13.7 7.6h.01", false, 2), ("M11.1 9.1h1.8L12 10.3z", true, 1), ("M8.6 20.6h2.4M13 20.6h2.4", false, 1.6)],
    ]
    private static var cache: [Int: [(NSBezierPath, Bool, CGFloat)]] = [:]

    static func draw(_ os: Int, in r: NSRect, color: NSColor) {
        let parts = cache[os] ?? (paths[os] ?? paths[1]!).map { (parse($0.0), $0.1, $0.2) }
        cache[os] = parts
        let s = r.width / 24
        let t = AffineTransform(translationByX: r.minX, byY: r.minY)
        for (path, fill, width) in parts {
            let p = path.copy() as! NSBezierPath
            p.transform(using: AffineTransform(scale: s)); p.transform(using: t)
            p.lineWidth = width * s; p.lineCapStyle = .round; p.lineJoinStyle = .round
            color.set()
            if fill { p.fill() } else { p.stroke() }
        }
    }

    /// 只认这几个图形里用到的 SVG 路径命令：M L H V C S Z（大小写都行）
    static func parse(_ d: String) -> NSBezierPath {
        var nums: [CGFloat] = [], ops: [(Character, [CGFloat])] = []
        var cmd: Character = "M", started = false
        let chars = Array(d)
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c.isLetter { if started { ops.append((cmd, nums)); nums = [] }; started = true; cmd = c; i += 1; continue }
            if c == " " || c == "," { i += 1; continue }
            // 一个数：可以带正负号，“.5”这种省了 0 的也算；“1.1.2”是 1.1 和 .2 两个数
            var j = i, dot = false, text = ""
            if chars[j] == "-" || chars[j] == "+" { text.append(chars[j]); j += 1 }
            while j < chars.count, chars[j].isASCII, chars[j].isNumber || (chars[j] == "." && !dot) {
                if chars[j] == "." { dot = true; if text.isEmpty || text == "-" || text == "+" { text.append("0") } }
                text.append(chars[j]); j += 1
            }
            if j == i { i += 1; continue }
            nums.append(CGFloat(Double(text) ?? 0)); i = j
        }
        if started { ops.append((cmd, nums)) }
        let p = NSBezierPath()
        var cur = CGPoint.zero, start = CGPoint.zero, ctrl = CGPoint.zero, lastWasCurve = false
        for (op, n) in ops {
            let rel = op.isLowercase
            let base = { (k: Int) -> CGPoint in rel ? CGPoint(x: cur.x + n[k], y: cur.y + n[k + 1]) : CGPoint(x: n[k], y: n[k + 1]) }
            switch op.uppercased() {
            case "M":
                var k = 0
                while k + 1 < n.count { let q = base(k); if k == 0 { p.move(to: q); start = q } else { p.line(to: q) }; cur = q; k += 2 }
                lastWasCurve = false
            case "L":
                var k = 0
                while k + 1 < n.count { let q = base(k); p.line(to: q); cur = q; k += 2 }
                lastWasCurve = false
            case "H": for v in n { cur = CGPoint(x: rel ? cur.x + v : v, y: cur.y); p.line(to: cur) }; lastWasCurve = false
            case "V": for v in n { cur = CGPoint(x: cur.x, y: rel ? cur.y + v : v); p.line(to: cur) }; lastWasCurve = false
            case "C":
                var k = 0
                while k + 5 < n.count {
                    let c1 = base(k), c2 = base(k + 2), q = base(k + 4)
                    p.curve(to: q, controlPoint1: c1, controlPoint2: c2); ctrl = c2; cur = q; k += 6
                }
                lastWasCurve = true
            case "S":
                var k = 0
                while k + 3 < n.count {
                    let c1 = lastWasCurve ? CGPoint(x: 2 * cur.x - ctrl.x, y: 2 * cur.y - ctrl.y) : cur
                    let c2 = base(k), q = base(k + 2)
                    p.curve(to: q, controlPoint1: c1, controlPoint2: c2); ctrl = c2; cur = q; k += 4; lastWasCurve = true
                }
            case "Z": p.close(); cur = start; lastWasCurve = false
            default: break
            }
        }
        return p
    }
}

final class MapView: NSView {
    enum Mode { case mini, board }
    let mode: Mode
    var onDrop: ((String, Int, Int) -> Void)?
    private var drag: (id: String, at: NSPoint, grab: NSPoint)?

    init(_ mode: Mode) {
        self.mode = mode
        super.init(frame: NSRect(x: 0, y: 0, width: 264, height: mode == .mini ? 74 : 240))
        translatesAutoresizingMaskIntoConstraints = false
        heightAnchor.constraint(equalToConstant: frame.height).isActive = true
        if mode == .mini { registerForDraggedTypes([.fileURL]) }
    }
    required init?(coder: NSCoder) { fatalError() }
    override var isFlipped: Bool { true }

    private struct Card { var id: String; var os: Int; var label: String; var x: Int; var y: Int; var me: Bool; var online: Bool; var lonely: Bool }
    private var cards: [Card] {
        let s = Share.shared
        var list = s.cells.map { c -> Card in
            let m = s.machines[c.id]
            let me = c.id == s.me
            return Card(id: c.id, os: me ? 1 : (m?.os ?? 0), label: "", x: c.x, y: c.y, me: me, online: me || (m?.online ?? false), lonely: false)
        }
        if !list.contains(where: { $0.me }) { list.append(Card(id: s.me, os: 1, label: "", x: 0, y: 0, me: true, online: true, lonely: false)) }
        let at = { (x: Int, y: Int) in list.contains { $0.x == x && $0.y == y } }
        for i in list.indices {
            let c = list[i]
            list[i].lonely = list.count > 1 && ![(1, 0), (-1, 0), (0, 1), (0, -1)].contains { at(c.x + $0.0, c.y + $0.1) }
            let osName = ["电脑", "macOS", "Windows", "Linux"][max(0, min(3, c.os))]
            if mode == .mini { list[i].label = c.me ? "这台" : osName }
            else { list[i].label = c.me ? "这台电脑" : (s.machines[c.id]?.name ?? "另一台电脑") }
        }
        return list
    }

    /// 格子大小和左上角的格子坐标
    private func grid(_ list: [Card]) -> (cw: CGFloat, ch: CGFloat, x0: Int, y0: Int, ox: CGFloat, oy: CGFloat) {
        let xs = list.map(\.x), ys = list.map(\.y)
        let x0 = xs.min() ?? 0, x1 = xs.max() ?? 0, y0 = ys.min() ?? 0, y1 = ys.max() ?? 0
        let margin: CGFloat = mode == .board ? 1 : 0                        // 大图四周各留半格，拖到那里就是往外加一格
        let cols = CGFloat(x1 - x0 + 1) + margin, rows = CGFloat(y1 - y0 + 1) + margin
        let (bw, bh): (CGFloat, CGFloat) = mode == .mini ? (58, 64) : (74, 86)
        let scale = min(1, bounds.width / (cols * bw), bounds.height / (rows * bh))
        let cw = bw * scale, ch = bh * scale
        return (cw, ch, x0, y0, (bounds.width - cols * cw) / 2 + margin * cw / 2, (bounds.height - rows * ch) / 2 + margin * ch / 2)
    }

    override func draw(_ dirty: NSRect) {
        let list = cards
        let g = grid(list)
        let sq = (mode == .mini ? 42 : 52) * g.cw / (mode == .mini ? 58 : 74)
        func origin(_ c: Card) -> NSPoint {
            if let d = drag, d.id == c.id { return NSPoint(x: d.at.x - d.grab.x, y: d.at.y - d.grab.y) }
            return NSPoint(x: g.ox + CGFloat(c.x - g.x0) * g.cw, y: g.oy + CGFloat(c.y - g.y0) * g.ch)
        }
        func square(_ c: Card) -> NSRect { let o = origin(c); return NSRect(x: o.x + (g.cw - sq) / 2, y: o.y + 4, width: sq, height: sq) }
        // 挨着、而且都连着（键鼠共享开着）的两台之间画黄色连线：鼠标能从这一侧穿过去。没连上的不画
        do {
            Palette.gold.set()
            for a in list where drag?.id != a.id && a.online {
                for b in list where drag?.id != b.id && b.online {
                    let ra = square(a), rb = square(b)
                    if b.x == a.x + 1 && b.y == a.y { NSBezierPath(roundedRect: NSRect(x: ra.maxX + 2, y: ra.midY - 2, width: rb.minX - ra.maxX - 4, height: 4), xRadius: 2, yRadius: 2).fill() }
                    // 上下挨着的连线画在卡片左侧，免得压住下面的名字
                    if b.y == a.y + 1 && b.x == a.x { NSBezierPath(roundedRect: NSRect(x: ra.minX + 4, y: ra.maxY + 3, width: 4, height: rb.minY - ra.maxY - 6), xRadius: 2, yRadius: 2).fill() }
                }
            }
        }
        for c in list.filter({ $0.id != drag?.id }) + list.filter({ $0.id == drag?.id }) {     // 拖着的那张画在最上面
            let r = square(c)
            let alpha: CGFloat = c.online ? 1 : 0.45
            if drag?.id == c.id {
                NSGraphicsContext.saveGraphicsState()
                let sh = NSShadow(); sh.shadowBlurRadius = 12; sh.shadowOffset = NSSize(width: 0, height: -5); sh.shadowColor = NSColor.black.withAlphaComponent(0.25); sh.set()
                Palette.card.set(); NSBezierPath(roundedRect: r, xRadius: sq * 0.22, yRadius: sq * 0.22).fill()
                NSGraphicsContext.restoreGraphicsState()
            }
            let box = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: sq * 0.22, yRadius: sq * 0.22)
            Palette.card.withAlphaComponent(alpha).set(); box.fill()
            let lonely = c.lonely && mode == .board
            if c.me { Palette.gold.set(); box.lineWidth = mode == .mini ? 2 : 2.5 }
            else if lonely { Palette.red.set(); box.lineWidth = 1.5; box.setLineDash([4, 3], count: 2, phase: 0) }
            else { Palette.border.withAlphaComponent(alpha).set(); box.lineWidth = 1 }
            box.stroke()
            if mode == .mini && c.id == Share.shared.selected {             // 左键选中的：一圈蓝色
                let h = NSBezierPath(roundedRect: r.insetBy(dx: -3, dy: -3), xRadius: sq * 0.26, yRadius: sq * 0.26)
                NSColor.controlAccentColor.set(); h.lineWidth = 2; h.stroke()
            }
            if c.id == dropTarget {                                         // 文件拖到这张卡片上了：虚线黄框
                let h = NSBezierPath(roundedRect: r.insetBy(dx: -3, dy: -3), xRadius: sq * 0.26, yRadius: sq * 0.26)
                Palette.gold.set(); h.lineWidth = 2; h.setLineDash([4, 3], count: 2, phase: 0); h.stroke()
            }
            let gs = sq * (mode == .mini ? 0.48 : 0.54)
            Glyph.draw(c.os, in: NSRect(x: r.midX - gs / 2, y: r.midY - gs / 2, width: gs, height: gs), color: (lonely ? Palette.red : NSColor.labelColor).withAlphaComponent(alpha))
            let para = NSMutableParagraphStyle(); para.alignment = .center; para.lineBreakMode = .byTruncatingTail
            let size: CGFloat = mode == .mini ? 10.5 : 11.5
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: size, weight: c.me ? .semibold : .regular),
                                                        .foregroundColor: c.me ? NSColor.labelColor : NSColor.secondaryLabelColor, .paragraphStyle: para]
            let lw = max(g.cw - 2, sq + 16)
            (c.label as NSString).draw(in: NSRect(x: r.midX - lw / 2, y: r.maxY + 2, width: lw, height: size + 4), withAttributes: attrs)
            if mode == .board && !c.online {
                var dim = attrs; dim[.font] = NSFont.systemFont(ofSize: 10.5); dim[.foregroundColor] = NSColor.tertiaryLabelColor
                ("未连接" as NSString).draw(in: NSRect(x: r.midX - g.cw / 2, y: r.maxY + size + 5, width: g.cw, height: 14), withAttributes: dim)
            }
        }
    }

    // 拖动：只有大图能拖
    override var acceptsFirstResponder: Bool { mode == .mini }
    override func mouseDown(with e: NSEvent) {
        if mode == .mini {
            // 左键选中一台连着的电脑（再点一下取消），之后按 ⌘V 把复制的文件发给它
            let c = card(at: convert(e.locationInWindow, from: nil))
            // 选中状态下再点一次选中的那台：取消选中（下拉不收起，恢复成点别处就收起）
            if let c, c.id == Share.shared.selected { Share.shared.selected = nil; needsDisplay = true; return }
            if let c, !c.me, c.online { Share.shared.selected = c.id; window?.makeFirstResponder(self); needsDisplay = true }
            return
        }
        guard mode == .board else { return }
        let p = convert(e.locationInWindow, from: nil)
        let list = cards, g = grid(list)
        for c in list {
            let o = NSPoint(x: g.ox + CGFloat(c.x - g.x0) * g.cw, y: g.oy + CGFloat(c.y - g.y0) * g.ch)
            if NSRect(origin: o, size: NSSize(width: g.cw, height: g.ch)).contains(p) { drag = (c.id, p, NSPoint(x: p.x - o.x, y: p.y - o.y)); NSCursor.closedHand.set(); needsDisplay = true; return }
        }
    }
    override func mouseDragged(with e: NSEvent) {
        guard var d = drag else { return }
        d.at = convert(e.locationInWindow, from: nil); drag = d; needsDisplay = true
    }
    override func mouseUp(with e: NSEvent) {
        guard let d = drag else { return }
        let list = cards, g = grid(list)
        let x = g.x0 + Int(((d.at.x - d.grab.x - g.ox) / g.cw).rounded()), y = g.y0 + Int(((d.at.y - d.grab.y - g.oy) / g.ch).rounded())
        drag = nil; NSCursor.openHand.set(); needsDisplay = true
        if let c = list.first(where: { $0.id == d.id }), (c.x, c.y) != (x, y) { onDrop?(d.id, x, y) }
    }
    override func resetCursorRects() { if mode == .board { addCursorRect(bounds, cursor: .openHand) } }

    private func card(at p: NSPoint) -> Card? {
        let list = cards, g = grid(list)
        return list.first { c in NSRect(x: g.ox + CGFloat(c.x - g.x0) * g.cw, y: g.oy + CGFloat(c.y - g.y0) * g.ch, width: g.cw, height: g.ch).contains(p) }
    }
    // 右键（或按住 Control 点）卡片：小图上，别的电脑“发送文件…”，自己“收到的文件保存到…”；大图上，别的电脑“请出这一组”
    override func menu(for e: NSEvent) -> NSMenu? {
        guard let c = card(at: convert(e.locationInWindow, from: nil)) else { return nil }
        let m = NSMenu()
        func add(_ title: String, _ sel: Selector) { let i = NSMenuItem(title: title, action: sel, keyEquivalent: ""); i.target = self; i.representedObject = c.id; m.addItem(i) }
        if mode == .board { if !c.me { add("把“\(Share.shared.machines[c.id]?.name ?? c.label)”请出这一组", #selector(forget(_:))) } }
        else if c.me { add("保存位置", #selector(saveDir(_:))) }
        else if c.online { add("发送文件", #selector(sendTo(_:))) }
        return m.items.isEmpty ? nil : m
    }
    override func performKeyEquivalent(with e: NSEvent) -> Bool {
        guard mode == .mini, e.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command, e.charactersIgnoringModifiers == "v",
              let id = Share.shared.selected else { return super.performKeyEquivalent(with: e) }
        let urls = (NSPasteboard.general.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        slog("⌘V：剪贴板里 \(urls.count) 个文件")
        if urls.isEmpty { NSSound.beep(); return true }                     // 剪贴板里没有文件
        Share.shared.send(urls, to: id)
        return true
    }
    @objc private func forget(_ item: NSMenuItem) {
        guard let id = item.representedObject as? String else { return }
        let a = NSAlert()
        a.messageText = "把“\(Share.shared.machines[id]?.name ?? "这台电脑")”请出这一组？"
        a.informativeText = "这一组会换一个新的校验码，连着的组员自动跟着换；它拿着旧的进不来。现在没开机的其他电脑，之后也要用新校验码重新加入。"
        a.addButton(withTitle: "请出去"); a.addButton(withTitle: "取消")
        NSApp.activate(ignoringOtherApps: true)
        if a.runModal() == .alertFirstButtonReturn { Share.shared.forget(id) }
    }
    @objc private func sendTo(_ item: NSMenuItem) { if let id = item.representedObject as? String { Share.shared.chooseAndSend(to: id) } }
    @objc private func saveDir(_ item: NSMenuItem) { Share.shared.chooseRecvDir() }

    // 把文件拖到小图里别的电脑的卡片上：发给它
    private var dropTarget: String? { didSet { if dropTarget != oldValue { needsDisplay = true } } }
    /// 拖到哪张卡片上就发给哪台；没对准卡片时，发给左键选中的那台
    private func target(_ s: NSDraggingInfo) -> String? {
        guard mode == .mini else { return nil }
        if let c = card(at: convert(s.draggingLocation, from: nil)), !c.me, c.online { return c.id }
        return Share.shared.selected
    }
    override func draggingEntered(_ s: NSDraggingInfo) -> NSDragOperation { dropTarget = target(s); return dropTarget == nil ? [] : .copy }
    override func draggingUpdated(_ s: NSDraggingInfo) -> NSDragOperation { dropTarget = target(s); return dropTarget == nil ? [] : .copy }
    override func draggingExited(_ s: NSDraggingInfo?) { dropTarget = nil }
    override func performDragOperation(_ s: NSDraggingInfo) -> Bool {
        slog("文件拖到小图上（对准：\(target(s) ?? "无")）")
        defer { dropTarget = nil }
        guard let id = target(s) else { return false }
        let urls = (s.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
        Share.shared.send(urls, to: id)
        return !urls.isEmpty
    }
}

// MARK: 下拉界面里“键鼠共享”那一块

final class SharePanel {
    let toggle = SwitchRow("键鼠共享")
    private let permRow: ItemRow
    private let mini = MapView(.mini)
    private let board = MapView(.board)
    private let well = NSView()
    private let wellStack = NSStackView()
    private let capLabel = NSTextField(labelWithString: "")
    private let sizeButton = NSButton(title: "展开", target: nil, action: nil)
    private let say = NSTextField(wrappingLabelWithString: "")
    private let foundStack = NSStackView()
    private let transferStack = NSStackView()
    private var jobRows: [String: JobRow] = [:]
    private let moreLabel = NSTextField(labelWithString: "")
    private let addrLabel = NSTextField(labelWithString: "")
    var expanded = false
    private var capRow = NSView(), addrRow = NSView(), codeRow = NSView(), backRow = NSView()
    private let backSwitch = NSSwitch()
    private let codeLabel = NSTextField(labelWithString: "")
    /// 灰底那块里只放现在要显示的几行（靠“隐藏”的话，系统有时不把空位让出来，收起后还是那么高）
    private func setRows(_ rows: [NSView]) {
        guard wellStack.arrangedSubviews != rows else { return }
        for v in wellStack.arrangedSubviews { wellStack.removeArrangedSubview(v); v.removeFromSuperview() }
        for v in rows { wellStack.addArrangedSubview(v); v.widthAnchor.constraint(equalTo: wellStack.widthAnchor).isActive = true }
    }
    var onResize: (() -> Void)?
    /// 下拉面板里要依次放进去的几行
    var views: [NSView] { [toggle, permRow, well] }

    init() {
        permRow = ItemRow("需要辅助功能权限", "去设置 ›") { Share.shared.askPermission(); Share.shared.openPermissionSettings() }
        sizeButton.target = self; sizeButton.action = #selector(flip)
        sizeButton.isBordered = false; sizeButton.font = .systemFont(ofSize: 12, weight: .semibold); sizeButton.contentTintColor = .labelColor
        capLabel.font = .systemFont(ofSize: 12); capLabel.textColor = .secondaryLabelColor
        say.font = .systemFont(ofSize: 12); say.textColor = .secondaryLabelColor; say.preferredMaxLayoutWidth = 262
        addrLabel.font = .systemFont(ofSize: 11.5); addrLabel.textColor = .secondaryLabelColor; addrLabel.isSelectable = true
        foundStack.orientation = .vertical; foundStack.alignment = .leading; foundStack.spacing = 6
        transferStack.orientation = .vertical; transferStack.alignment = .leading; transferStack.spacing = 6
        moreLabel.font = .systemFont(ofSize: 11.5); moreLabel.textColor = .secondaryLabelColor

        let cap = NSStackView(views: [capLabel, sizeButton]); cap.orientation = .horizontal; cap.distribution = .equalSpacing
        let manual = NSButton(title: "手动添加", target: self, action: #selector(manualAdd))
        manual.isBordered = false; manual.font = .systemFont(ofSize: 11.5, weight: .semibold); manual.contentTintColor = .labelColor
        let addr = NSStackView(views: [addrLabel, manual]); addr.orientation = .horizontal; addr.distribution = .equalSpacing
        wellStack.orientation = .vertical; wellStack.alignment = .leading; wellStack.spacing = 6
        codeLabel.font = .systemFont(ofSize: 11.5); codeLabel.textColor = .secondaryLabelColor; codeLabel.isSelectable = true
        let renew = NSButton(title: "换一个", target: self, action: #selector(newCode))
        renew.isBordered = false; renew.font = .systemFont(ofSize: 11.5, weight: .semibold); renew.contentTintColor = .labelColor
        renew.toolTip = "换一个新的校验码：连着的组员自动跟着换，没开机的电脑之后要用新码重新加入"
        let code = NSStackView(views: [codeLabel, renew]); code.orientation = .horizontal; code.distribution = .equalSpacing
        let backLabel = NSTextField(labelWithString: "鼠标被控切回"); backLabel.font = .systemFont(ofSize: 12)
        backSwitch.controlSize = .mini; backSwitch.target = self; backSwitch.action = #selector(flipBack)
        let back = NSStackView(views: [backLabel, backSwitch]); back.orientation = .horizontal; back.distribution = .equalSpacing
        capRow = cap; addrRow = addr; codeRow = code; backRow = back
        wellStack.translatesAutoresizingMaskIntoConstraints = false
        well.wantsLayer = true; well.layer?.cornerRadius = 8
        well.addSubview(wellStack)
        NSLayoutConstraint.activate([wellStack.leadingAnchor.constraint(equalTo: well.leadingAnchor, constant: 10), wellStack.trailingAnchor.constraint(equalTo: well.trailingAnchor, constant: -10),
                                     wellStack.topAnchor.constraint(equalTo: well.topAnchor, constant: 10), wellStack.bottomAnchor.constraint(equalTo: well.bottomAnchor, constant: -8)])
        board.onDrop = { id, x, y in Share.shared.place(id, x, y) }
    }

    @objc private func flip() { expanded.toggle(); relayout() }
    @objc private func flipBack() { Share.shared.setTakeBack(backSwitch.state == .on) }
    /// 展开、收起后下拉框要跟着变高矮：交给外面整个重排
    private func relayout() { if let f = onResize { f() } else { refresh() } }

    /// 问用户要校验码（withAddress：手动添加，多一行地址）。返回（地址，校验码）；取消返回 nil
    private func askCode(title: String, withAddress: Bool) -> (String, String)? {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = "输入它的面板里显示的校验码（点“展开”能看到）。" + (Share.shared.connected > 0 ? "\n加入后这台会离开现在的组，原来组里的其他电脑不会跟着过去。" : "")
        let addr = NSTextField(frame: NSRect(x: 0, y: 30, width: 240, height: 24)); addr.placeholderString = "地址，例如 192.168.1.20"
        let code = NSTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 24)); code.placeholderString = "校验码，例如 K7M4-P2XQ"
        code.font = .monospacedSystemFont(ofSize: 14, weight: .semibold)
        let box = NSView(frame: NSRect(x: 0, y: 0, width: 240, height: withAddress ? 54 : 24))
        if withAddress { box.addSubview(addr); addr.nextKeyView = code }
        box.addSubview(code)
        a.accessoryView = box
        a.addButton(withTitle: "加入"); a.addButton(withTitle: "取消")
        a.window.initialFirstResponder = withAddress ? addr : code
        NSApp.activate(ignoringOtherApps: true)
        guard a.runModal() == .alertFirstButtonReturn else { return nil }
        // 没填校验码就当没点
        guard !code.stringValue.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return (addr.stringValue, code.stringValue)
    }
    @objc private func manualAdd() {
        guard let (addr, code) = askCode(title: "手动添加：加入另一台电脑所在的组", withAddress: true) else { return }
        if !Share.shared.connect(address: addr, code: code) {
            let e = NSAlert(); e.messageText = "地址不对"; e.informativeText = "例如 192.168.1.20"; e.runModal()
        }
    }
    @objc private func newCode() {
        let a = NSAlert()
        a.messageText = "换一个校验码？"
        a.informativeText = "连着的组员会自动跟着换。现在没开机的电脑，之后要用新的校验码重新加入。"
        a.addButton(withTitle: "换"); a.addButton(withTitle: "取消")
        NSApp.activate(ignoringOtherApps: true)
        if a.runModal() == .alertFirstButtonReturn { Share.shared.newCode() }
    }

    func refresh() {
        let s = Share.shared, on = s.running
        toggle.toggle.state = on ? .on : .off
        // “安全输入”的提示：那个程序已经关掉了就不再显示
        let problem = s.problem == Share.secureText && !IsSecureEventInputEnabled() ? nil : s.problem
        let n = s.connected
        if !on { toggle.sub.stringValue = problem ?? "已关闭" }
        else if let p = problem { toggle.sub.stringValue = p }
        else if !s.trusted { toggle.sub.stringValue = "需要辅助功能权限" }
        else { toggle.sub.stringValue = n > 0 ? "已连接 \(n) 台" : "未连接" }
        permRow.isHidden = !on || s.trusted
        well.isHidden = !on
        well.layer?.backgroundColor = Palette.well.cgColor
        let warn = sentence()
        let jobs: [NSView] = s.transfers.isEmpty ? [] : [transferStack]
        setRows(expanded ? [board, capRow] + (warn.isEmpty ? [] : [say]) + [codeRow] + (s.found.isEmpty ? [] : [foundStack]) + [addrRow, backRow] + jobs : [mini, capRow] + jobs)
        backSwitch.state = s.takeBack ? .on : .off
        // 进度行按编号复用（每 0.2 秒更新一次，重建的话 ✕ 会在按下和松开之间被换掉，点不上）
        let shown = Array(s.transfers.suffix(3)), more = s.transfers.count - shown.count
        for k in jobRows.keys where !shown.contains(where: { $0.key == k }) { jobRows[k]?.removeFromSuperview(); jobRows[k] = nil }
        for (i, t) in shown.enumerated() {
            let row = jobRows[t.key] ?? { let r = JobRow(key: t.key, target: self, action: #selector(cancelJob(_:))); jobRows[t.key] = r; return r }()
            if row.superview == nil || transferStack.arrangedSubviews.firstIndex(of: row) != i {
                row.removeFromSuperview(); transferStack.insertArrangedSubview(row, at: min(i, transferStack.arrangedSubviews.count))
                row.widthAnchor.constraint(equalTo: transferStack.widthAnchor).isActive = true
            }
            row.show(t)
        }
        moreLabel.stringValue = more > 0 ? "还有 \(more) 个" : ""
        if more > 0 { if moreLabel.superview == nil { transferStack.addArrangedSubview(moreLabel) } } else { moreLabel.removeFromSuperview() }
        capLabel.stringValue = expanded ? "拖动卡片摆放" : (s.selected.map { "拖文件进来，发给\(s.machines[$0]?.name ?? "它")" } ?? "")
        // 收起时只有一个“展开”；发现了新电脑，旁边亮一个小红点
        let title = NSMutableAttributedString(string: expanded ? "收起" : "展开", attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .semibold), .foregroundColor: NSColor.labelColor])
        if !expanded && !s.found.isEmpty { title.append(NSAttributedString(string: " ●", attributes: [.font: NSFont.systemFont(ofSize: 7), .foregroundColor: NSColor.systemRed, .baselineOffset: 4])) }
        sizeButton.attributedTitle = title
        addrLabel.stringValue = "本机 " + (s.address ?? "没连网")
        let gc = s.groupCode
        codeLabel.stringValue = "校验码 " + (gc.count == 8 ? gc.prefix(4) + "-" + gc.suffix(4) : gc)
        say.stringValue = sentence()
        say.textColor = sentenceWarns ? Palette.red : .secondaryLabelColor
        foundStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for f in s.found {
            let l = NSTextField(labelWithString: f.name); l.font = .systemFont(ofSize: 12.5); l.lineBreakMode = .byTruncatingTail
            let b = NSButton(title: f.joining ? "等对方同意…" : "加入", target: self, action: #selector(join(_:)))
            b.bezelStyle = .rounded; b.controlSize = .small; b.identifier = NSUserInterfaceItemIdentifier(f.id); b.isEnabled = !f.joining
            let row = NSStackView(views: [l, b]); row.orientation = .horizontal; row.distribution = .equalSpacing
            foundStack.addArrangedSubview(row); row.widthAnchor.constraint(equalTo: foundStack.widthAnchor).isActive = true
        }
        mini.needsDisplay = true; board.needsDisplay = true
    }
    @objc private func cancelJob(_ b: NSButton) { if let k = b.identifier?.rawValue { Share.shared.cancel(k) } }
    @objc private func join(_ b: NSButton) {
        guard let id = b.identifier?.rawValue, let f = Share.shared.found.first(where: { $0.id == id }) else { return }
        guard let (_, code) = askCode(title: "加入“\(f.name)”所在的组", withAddress: false) else { return }
        Share.shared.join(id, code: code)
    }

    private var sentenceWarns = false
    /// 大图下面那句话：只在有问题时说（谁没挨着、鼠标到不了），没问题就不说
    private func sentence() -> String {
        let s = Share.shared
        sentenceWarns = false
        guard s.cells.count >= 2, s.cells.contains(where: { $0.id == s.me }) else { return "其他电脑打开键鼠共享后会出现在这里" }
        let at = { (x: Int, y: Int) in s.cells.contains { $0.x == x && $0.y == y } }
        let lonely = s.cells.filter { c in ![(1, 0), (-1, 0), (0, 1), (0, -1)].contains { at(c.x + $0.0, c.y + $0.1) } }
        guard !lonely.isEmpty else { return "" }
        sentenceWarns = true
        return lonely.map { $0.id == s.me ? "这台" : "「\(s.machines[$0.id]?.name ?? "另一台电脑")」" }.joined(separator: "、") + "没挨着别的电脑，鼠标到不了"
    }
}
