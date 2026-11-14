// 一苇 · 截图：选区、标注、工具条
import Cocoa

enum Tool: Int { case rect, ellipse, arrow, pen, mosaic, text }

struct Mark {
    var tool: Tool
    var points: [NSPoint]      // 矩形、椭圆、箭头、马赛克：[起点, 终点]；画笔：整条轨迹
    var color: NSColor
    var width: CGFloat         // 线宽；文字标注时是字号
    var text = ""              // 文字标注的内容，points[0] 是文字左上角
}

/// 把截好的图放进剪贴板，需要时另存成文件。普通截图和长截屏共用。
func deliverShot(_ cg: CGImage, pointSize: NSSize, save: Bool, dir: String) {
    let rep = NSBitmapImageRep(cgImage: cg)
    rep.size = pointSize
    let img = NSImage(size: pointSize); img.addRepresentation(rep)
    let pb = NSPasteboard.general
    pb.clearContents(); pb.writeObjects([img])
    if save, let data = rep.representation(using: .png, properties: [:]) {
        let path = NSString(string: dir).expandingTildeInPath
        try? FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd HH.mm.ss"
        try? data.write(to: URL(fileURLWithPath: path).appendingPathComponent("一苇截图 \(f.string(from: Date())).png"))
    }
}

let ywGold = NSColor(srgbRed: 0.95, green: 0.72, blue: 0.02, alpha: 1)
let ywInk = NSColor(srgbRed: 0.10, green: 0.10, blue: 0.10, alpha: 1)

// MARK: 工具条图标（全部用路径画，不依赖图片文件）

func toolIcon(_ name: String) -> NSImage {
    let img = NSImage(size: NSSize(width: 20, height: 20), flipped: true) { _ in
        NSColor.black.setStroke(); NSColor.black.setFill()
        let p = NSBezierPath(); p.lineWidth = 1.6; p.lineCapStyle = .round; p.lineJoinStyle = .round
        switch name {
        case "rect": p.appendRoundedRect(NSRect(x: 3, y: 4, width: 14, height: 12), xRadius: 1, yRadius: 1); p.stroke()
        case "ellipse": p.appendOval(in: NSRect(x: 3, y: 4, width: 14, height: 12)); p.stroke()
        case "arrow": p.move(to: NSPoint(x: 4, y: 16)); p.line(to: NSPoint(x: 16, y: 4)); p.move(to: NSPoint(x: 9, y: 4)); p.line(to: NSPoint(x: 16, y: 4)); p.line(to: NSPoint(x: 16, y: 11)); p.stroke()
        case "pen": p.move(to: NSPoint(x: 3, y: 15)); p.curve(to: NSPoint(x: 10, y: 13), controlPoint1: NSPoint(x: 6, y: 8), controlPoint2: NSPoint(x: 8, y: 17)); p.curve(to: NSPoint(x: 17, y: 6), controlPoint1: NSPoint(x: 12, y: 8), controlPoint2: NSPoint(x: 13, y: 7)); p.stroke()
        case "mosaic":
            NSBezierPath(rect: NSRect(x: 3, y: 3, width: 6.5, height: 6.5)).fill(); NSBezierPath(rect: NSRect(x: 10.5, y: 10.5, width: 6.5, height: 6.5)).fill()
            NSColor.black.withAlphaComponent(0.3).setFill()
            NSBezierPath(rect: NSRect(x: 10.5, y: 3, width: 6.5, height: 6.5)).fill(); NSBezierPath(rect: NSRect(x: 3, y: 10.5, width: 6.5, height: 6.5)).fill()
        case "text": p.move(to: NSPoint(x: 4.5, y: 5)); p.line(to: NSPoint(x: 15.5, y: 5)); p.move(to: NSPoint(x: 10, y: 5)); p.line(to: NSPoint(x: 10, y: 16)); p.stroke()
        case "ocr":       // 四个角加一个“文”
            for (a, b, c) in [((3.0, 6.0), (3.0, 3.0), (6.0, 3.0)), ((14, 3), (17, 3), (17, 6)), ((17, 14), (17, 17), (14, 17)), ((6, 17), (3, 17), (3, 14))] {
                p.move(to: NSPoint(x: a.0, y: a.1)); p.line(to: NSPoint(x: b.0, y: b.1)); p.line(to: NSPoint(x: c.0, y: c.1))
            }
            p.move(to: NSPoint(x: 6.5, y: 8)); p.line(to: NSPoint(x: 13.5, y: 8)); p.move(to: NSPoint(x: 10, y: 6)); p.line(to: NSPoint(x: 10, y: 8))
            p.move(to: NSPoint(x: 7.5, y: 8)); p.curve(to: NSPoint(x: 13, y: 13.6), controlPoint1: NSPoint(x: 8.1, y: 10.6), controlPoint2: NSPoint(x: 9.9, y: 12.6))
            p.move(to: NSPoint(x: 12.5, y: 8)); p.curve(to: NSPoint(x: 7, y: 13.6), controlPoint1: NSPoint(x: 11.9, y: 10.6), controlPoint2: NSPoint(x: 10.1, y: 12.6))
            p.stroke()
        case "long":
            p.appendRoundedRect(NSRect(x: 5, y: 2.5, width: 10, height: 15), xRadius: 2, yRadius: 2)
            p.move(to: NSPoint(x: 10, y: 6)); p.line(to: NSPoint(x: 10, y: 13.5)); p.move(to: NSPoint(x: 7.5, y: 11)); p.line(to: NSPoint(x: 10, y: 13.5)); p.line(to: NSPoint(x: 12.5, y: 11)); p.stroke()
        case "undo": p.move(to: NSPoint(x: 7, y: 4)); p.line(to: NSPoint(x: 3, y: 8)); p.line(to: NSPoint(x: 7, y: 12)); p.move(to: NSPoint(x: 3, y: 8)); p.line(to: NSPoint(x: 11, y: 8)); p.appendArc(withCenter: NSPoint(x: 11, y: 13), radius: 5, startAngle: 270, endAngle: 90, clockwise: false); p.line(to: NSPoint(x: 9, y: 18)); p.stroke()
        case "redo": p.move(to: NSPoint(x: 13, y: 4)); p.line(to: NSPoint(x: 17, y: 8)); p.line(to: NSPoint(x: 13, y: 12)); p.move(to: NSPoint(x: 17, y: 8)); p.line(to: NSPoint(x: 9, y: 8)); p.appendArc(withCenter: NSPoint(x: 9, y: 13), radius: 5, startAngle: 270, endAngle: 90, clockwise: true); p.line(to: NSPoint(x: 11, y: 18)); p.stroke()
        case "save": p.move(to: NSPoint(x: 10, y: 3)); p.line(to: NSPoint(x: 10, y: 13)); p.move(to: NSPoint(x: 6, y: 9)); p.line(to: NSPoint(x: 10, y: 13)); p.line(to: NSPoint(x: 14, y: 9)); p.move(to: NSPoint(x: 4, y: 17)); p.line(to: NSPoint(x: 16, y: 17)); p.stroke()
        case "cancel": p.move(to: NSPoint(x: 5, y: 5)); p.line(to: NSPoint(x: 15, y: 15)); p.move(to: NSPoint(x: 15, y: 5)); p.line(to: NSPoint(x: 5, y: 15)); p.stroke()
        case "done": p.lineWidth = 2; p.move(to: NSPoint(x: 4, y: 10.5)); p.line(to: NSPoint(x: 8, y: 14.5)); p.line(to: NSPoint(x: 16, y: 5.5)); p.stroke()
        default: break
        }
        return true
    }
    img.isTemplate = true
    return img
}

final class IconButton: NSButton {
    var onTap: (() -> Void)?
    private(set) var tip = ""
    private var host: OverlayView? { var v = superview; while let x = v, !(x is OverlayView) { v = x.superview }; return v as? OverlayView }
    var baseTint: NSColor = ywInk
    var fill: NSColor? { didSet { refresh() } }
    var isOn = false { didSet { refresh() } }
    private var hovering = false

    init(icon: String, tip: String, onTap: @escaping () -> Void) {
        super.init(frame: NSRect(x: 0, y: 0, width: 30, height: 28))
        self.onTap = onTap
        image = toolIcon(icon); imagePosition = .imageOnly; isBordered = false
        self.tip = tip; wantsLayer = true; layer?.cornerRadius = 6
        target = self; action = #selector(tapped)
        widthAnchor.constraint(equalToConstant: 30).isActive = true
        heightAnchor.constraint(equalToConstant: 28).isActive = true
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
        refresh()
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func tapped() { host?.showTip(nil); onTap?() }
    override func mouseEntered(with event: NSEvent) { hovering = true; refresh(); host?.showTip(self) }
    override func mouseExited(with event: NSEvent) { hovering = false; refresh(); host?.showTip(nil) }
    private func refresh() {
        if isOn { layer?.backgroundColor = ywInk.cgColor; contentTintColor = .white }
        else if let f = fill { layer?.backgroundColor = f.cgColor; contentTintColor = baseTint }
        else { layer?.backgroundColor = hovering ? NSColor(white: 0.93, alpha: 1).cgColor : NSColor.clear.cgColor; contentTintColor = baseTint }
    }
}

/// 点选时的一个候选：一个窗口。system 为真表示它是铺满整屏的系统层（通知中心、程序坞），
/// 真正能选的是里面的东西（通知横幅、程序坞那一条），要另外向系统问。
struct PickTarget { let rect: NSRect; let pid: pid_t; let system: Bool }

/// 悬停提示的小黑条
final class TipView: NSView {
    var text = "" { didSet { needsDisplay = true } }
    private let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11.5), .foregroundColor: NSColor.white]
    var fitting: NSSize { let s = (text as NSString).size(withAttributes: attrs); return NSSize(width: ceil(s.width) + 14, height: ceil(s.height) + 8) }
    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.08, alpha: 0.92).setFill()
        NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill()
        (text as NSString).draw(at: NSPoint(x: 7, y: 4), withAttributes: attrs)
    }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }      // 不挡鼠标
}

final class BarView: NSView {
    let stack = NSStackView()
    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.white.cgColor
        layer?.cornerRadius = 9
        shadow = NSShadow(); layer?.shadowColor = NSColor.black.cgColor; layer?.shadowOpacity = 0.25; layer?.shadowRadius = 8; layer?.shadowOffset = NSSize(width: 0, height: -2)
        stack.orientation = .horizontal; stack.spacing = 1; stack.edgeInsets = NSEdgeInsets(top: 4, left: 4, bottom: 4, right: 4)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor), stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor)])
    }
    required init?(coder: NSCoder) { fatalError() }
    // 点到按钮之间的空隙时到此为止，不让它落到下面的选区上
    override func mouseDown(with event: NSEvent) {}
    override func mouseDragged(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {}
    func divider() {
        let d = NSView(); d.wantsLayer = true; d.layer?.backgroundColor = NSColor(white: 0.88, alpha: 1).cgColor
        d.widthAnchor.constraint(equalToConstant: 1).isActive = true; d.heightAnchor.constraint(equalToConstant: 18).isActive = true
        stack.addArrangedSubview(d); stack.setCustomSpacing(5, after: d)
        if stack.arrangedSubviews.count > 1 { stack.setCustomSpacing(5, after: stack.arrangedSubviews[stack.arrangedSubviews.count - 2]) }
    }
}

/// 小条上带字的按钮（“全选”“复制”）
final class TextButton: NSButton {
    private var onTap: (() -> Void)?
    init(_ title: String, dark: Bool, onTap: @escaping () -> Void) {
        super.init(frame: .zero)
        self.onTap = onTap
        isBordered = false; wantsLayer = true; layer?.cornerRadius = 6
        layer?.backgroundColor = (dark ? ywInk : NSColor(white: 0.93, alpha: 1)).cgColor
        attributedTitle = NSAttributedString(string: title, attributes: [.font: NSFont.systemFont(ofSize: 12.5), .foregroundColor: dark ? NSColor.white : ywInk])
        target = self; action = #selector(tapped)
        widthAnchor.constraint(equalToConstant: ceil(attributedTitle.size().width) + 20).isActive = true
        heightAnchor.constraint(equalToConstant: 24).isActive = true
    }
    required init?(coder: NSCoder) { fatalError() }
    @objc private func tapped() { onTap?() }
}

final class DotButton: NSView {
    let color: NSColor; let dotSize: CGFloat; var isOn = false { didSet { needsDisplay = true } }
    var onTap: (() -> Void)?
    init(color: NSColor, dotSize: CGFloat, onTap: @escaping () -> Void) {
        self.color = color; self.dotSize = dotSize; self.onTap = onTap
        super.init(frame: NSRect(x: 0, y: 0, width: 24, height: 24))
        widthAnchor.constraint(equalToConstant: 24).isActive = true; heightAnchor.constraint(equalToConstant: 24).isActive = true
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ dirtyRect: NSRect) {
        let r = NSRect(x: bounds.midX - dotSize / 2, y: bounds.midY - dotSize / 2, width: dotSize, height: dotSize)
        color.setFill(); NSBezierPath(ovalIn: r).fill()
        NSColor(white: 0, alpha: 0.25).setStroke(); let edge = NSBezierPath(ovalIn: r); edge.lineWidth = 0.5; edge.stroke()
        if isOn { ywGold.setStroke(); let ring = NSBezierPath(ovalIn: r.insetBy(dx: -3, dy: -3)); ring.lineWidth = 2; ring.stroke() }
    }
    override func mouseDown(with event: NSEvent) { onTap?() }
}

// MARK: 截图会话

final class ShotSession {
    static var current: ShotSession?
    private(set) var windows: [OverlayWindow] = []
    let saveDir: String

    init(shots: [(NSScreen, CGImage)], saveDir: String) {
        self.saveDir = saveDir
        for (screen, image) in shots {
            let w = OverlayWindow(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            w.isFloatingPanel = true; w.hidesOnDeactivate = false; w.becomesKeyOnlyIfNeeded = false
            w.setFrame(screen.frame, display: false)
            w.level = .screenSaver; w.isOpaque = true; w.hasShadow = false; w.ignoresMouseEvents = false
            w.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            let v = OverlayView(image: image, frame: NSRect(origin: .zero, size: screen.frame.size))
            v.session = self
            v.targets = ShotSession.pickTargets(on: screen)
            v.screenFrame = screen.frame
            w.contentView = v
            windows.append(w)
        }
        previousApp = NSWorkspace.shared.frontmostApplication
        NSApp.activate(ignoringOtherApps: true)
        for w in windows { w.makeKeyAndOrderFront(nil); w.makeFirstResponder(w.contentView) }
        if let m = windows.first(where: { $0.frame.contains(NSEvent.mouseLocation) }) { m.makeKey() }
        NSCursor.crosshair.set()
        escKey = Hotkey(code: 53, mods: 0, id: 2) { ShotSession.current?.escape() }
    }
    private var escKey: Hotkey?
    private var long: LongShot?
    private var previousApp: NSRunningApplication?     // 截图前在最前面的应用；长截屏时把键盘焦点还给它

    /// 屏幕上各个窗口（从上到下），位置换算成这块屏幕自己的坐标。用来“鼠标指到哪个就框哪个”。
    static func pickTargets(on screen: NSScreen) -> [PickTarget] {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else { return [] }
        let primaryH = NSScreen.screens.first?.frame.height ?? screen.frame.height
        let me = ProcessInfo.processInfo.processIdentifier
        let full = screen.frame.width * screen.frame.height
        var out: [PickTarget] = []
        for w in list {
            let layer = (w[kCGWindowLayer as String] as? Int) ?? 0
            // 一苇自己的窗口也能选（比如菜单栏的下拉框），只是不选截图自己盖在屏幕上的那几层
            guard let pid = w[kCGWindowOwnerPID as String] as? Int32, pid != me || layer < Int(CGWindowLevelForKey(.screenSaverWindow)), ((w[kCGWindowAlpha as String] as? Double) ?? 1) > 0.05,
                  let b = w[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b as CFDictionary) else { continue }
            let cocoa = NSRect(x: r.minX, y: primaryH - r.maxY, width: r.width, height: r.height)
            let local = cocoa.offsetBy(dx: -screen.frame.minX, dy: -screen.frame.minY).intersection(NSRect(origin: .zero, size: screen.frame.size))
            guard local.width >= 16, local.height >= 16 else { continue }          // 菜单栏上的小图标也算
            // 普通应用窗口（第 0 层）都算；其它层的小窗口也算：桌面小组件、菜单、弹出框。
            if layer != 0 {
                // 系统自己的几层：菜单栏本身留着（可以整条框下来），别的（壁纸背后的垫底层等）不要
                if (w[kCGWindowOwnerName as String] as? String) == "Window Server", (w[kCGWindowName as String] as? String) != "Menubar" { continue }
                if local.width * local.height > full * 0.9 {
                    // 铺满整屏的层：在普通窗口上面的（通知中心、程序坞）留着，里面的东西另外问；壁纸、桌面那些不要
                    if layer > 0 { out.append(PickTarget(rect: local, pid: pid, system: true)) }
                    continue
                }
            }
            out.append(PickTarget(rect: local, pid: pid, system: false))
        }
        return out
    }

    /// Esc：正在打字就只取消这段字，否则取消整个截图。
    func escape() {
        for w in windows { if let v = w.contentView as? OverlayView, v.isEditingText { v.escapeText(); return } }
        end()
    }

    /// 进入长截屏：冻结的画面撤掉，露出真实的窗口让用户滚动。
    func startLong(screen: NSScreen, rect: NSRect) {
        for w in windows { w.orderOut(nil) }
        windows = []
        NSCursor.arrow.set()
        long = LongShot(screen: screen, rect: rect, saveDir: saveDir) { [weak self] in self?.end() }
        long?.previousApp = previousApp
        long?.start()
    }
    func claim(_ view: OverlayView) {
        for w in windows { if let v = w.contentView as? OverlayView, v !== view { v.active = false; v.needsDisplay = true } }
    }
    /// 没框出选区就松手：所有屏幕重新可选。
    func release() {
        for w in windows { if let v = w.contentView as? OverlayView { v.active = true; v.needsDisplay = true } }
    }
    func end() {
        escKey = nil
        long?.close(); long = nil
        for w in windows { w.orderOut(nil) }
        windows = []
        NSCursor.arrow.set()
        ShotSession.current = nil
    }
}

final class OverlayWindow: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

final class OverlayView: NSView {
    let image: CGImage
    weak var session: ShotSession?
    var targets: [PickTarget] = []       // 可以点选的各个窗口，上面的在前
    var screenFrame = NSRect.zero
    private var axCache: (at: TimeInterval, pid: pid_t, rect: NSRect?)?
    private var hover: NSRect?           // 还没框选时，鼠标下面那个窗口
    private var editor: NSTextField?     // 正在输入的文字标注
    private var editorTop = NSPoint.zero
    var isEditingText: Bool { editor != nil }
    var active = true
    var sel: NSRect?
    var tool: Tool? { didSet { if tool != nil { ocrOff() }; syncBars() } }   // 换工具后鼠标一动指针就跟着变；选了画标注的工具就退出“提取文字”

    // 提取文字：点了工具条上的按钮才识别；认出来的字可以在图上拖选、复制。这期间框不能动，标注工具停用
    private var ocrOn = false
    private var ocrToken = 0                              // 每次开始识别加一：晚到的旧结果不要
    private var glyphs: [(g: OCRGlyph, rect: NSRect)] = []   // 认出来的每个字和它在视图里的位置，按阅读顺序
    private var picked: (anchor: Int, focus: Int)?        // 选中的是从第几个字到第几个字（含两头）
    private var pressAt = NSPoint.zero                    // 选字时鼠标是在哪按下的：拖出去一小段才算开始选
    private var toast: String?
    private var toastToken = 0
    private let ocrBar = BarView()
    private let ocrLabel = NSTextField(labelWithString: "拖动选字")
    private var ocrButton: IconButton?
    var marks: [Mark] = []
    var redoStack: [Mark] = []
    var color = NSColor(srgbRed: 0.90, green: 0.21, blue: 0.17, alpha: 1)
    var lineWidth: CGFloat = 3

    private enum Drag { case none, selecting(NSPoint), moving(NSPoint, NSRect), resizing(Int, NSRect), drawing, picking }
    private var drag = Drag.none
    private let bar = BarView()
    private let styleBar = BarView()
    private let tipView = TipView()
    private var tipToken = 0
    private var toolButtons: [Tool: IconButton] = [:]
    private var colorDots: [DotButton] = []
    private var sizeDots: [DotButton] = []
    private let palette: [NSColor] = [NSColor(srgbRed: 0.90, green: 0.21, blue: 0.17, alpha: 1), ywGold, NSColor(srgbRed: 0.18, green: 0.62, blue: 0.35, alpha: 1), NSColor(srgbRed: 0.16, green: 0.45, blue: 0.90, alpha: 1), .black, .white]
    private let sizes: [CGFloat] = [2, 3, 6]

    init(image: CGImage, frame: NSRect) {
        self.image = image
        super.init(frame: frame)
        buildBars()
    }
    required init?(coder: NSCoder) { fatalError() }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    // MARK: 鼠标指针：工具条上是箭头，控制点上是对应方向的拉伸，选区里是手（画标注时是十字）
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for t in trackingAreas { removeTrackingArea(t) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .cursorUpdate, .activeAlways, .inVisibleRect], owner: self))
    }
    override func mouseMoved(with event: NSEvent) { updateCursor(convert(event.locationInWindow, from: nil)) }
    override func mouseEntered(with event: NSEvent) { updateCursor(convert(event.locationInWindow, from: nil)) }
    override func cursorUpdate(with event: NSEvent) { updateCursor(convert(event.locationInWindow, from: nil)) }

    private func resizeCursor(_ i: Int) -> NSCursor {
        let (dx, dy) = handleDirs[i]
        if dx == 0 { return .resizeUpDown }
        if dy == 0 { return .resizeLeftRight }
        if #available(macOS 15.0, *) {
            let pos: NSCursor.FrameResizePosition = dx < 0 ? (dy > 0 ? .topLeft : .bottomLeft) : (dy > 0 ? .topRight : .bottomRight)
            return NSCursor.frameResize(position: pos, directions: .all)
        }
        return .crosshair
    }
    private func updateCursor(_ p: NSPoint) {
        if sel == nil, !dragging {
            let h = active ? pick(at: p, fresh: false) : nil
            if h != hover { hover = h; needsDisplay = true }
        }
        guard active else { NSCursor.arrow.set(); return }
        let c: NSCursor
        if case .moving = drag { c = .closedHand }
        else if case .resizing(let i, _) = drag { c = resizeCursor(i) }
        else if case .picking = drag { c = .iBeam }
        else if dragging { c = .crosshair }
        else if (!bar.isHidden && bar.frame.contains(p)) || (!styleBar.isHidden && styleBar.frame.contains(p)) || (!ocrBar.isHidden && ocrBar.frame.contains(p)) { c = .arrow }
        else if ocrOn { c = glyphs.contains(where: { $0.rect.insetBy(dx: -4, dy: -4).contains(p) }) ? .iBeam : .arrow }
        else if let s = sel {
            if let i = handlePoints(s).firstIndex(where: { hypot($0.x - p.x, $0.y - p.y) <= 9 }) { c = resizeCursor(i) }
            else if s.contains(p) { c = tool != nil ? .crosshair : .openHand }
            else { c = marks.isEmpty ? .crosshair : .arrow }
        } else { c = .crosshair }
        c.set()
    }
    override func rightMouseDown(with event: NSEvent) { session?.end() }   // 右键取消
    override func mouseExited(with event: NSEvent) { if hover != nil { hover = nil; needsDisplay = true } }   // 鼠标去了别的屏幕

    private var dragging: Bool { if case .none = drag { return false }; return true }

    private func buildBars() {
        let tools: [(Tool, String, String)] = [(.rect, "rect", "矩形"), (.ellipse, "ellipse", "椭圆"), (.arrow, "arrow", "箭头"), (.pen, "pen", "画笔"), (.mosaic, "mosaic", "马赛克"), (.text, "text", "文字")]
        for (t, icon, tip) in tools {
            let b = IconButton(icon: icon, tip: tip) { [weak self] in guard let s = self else { return }; s.tool = (s.tool == t) ? nil : t }
            toolButtons[t] = b; bar.stack.addArrangedSubview(b)
        }
        let ocr = IconButton(icon: "ocr", tip: "提取文字：认出框里的字，拖动选中，⌘C 复制") { [weak self] in guard let s = self else { return }; if s.ocrOn { s.ocrOff() } else { s.ocrStart() } }
        ocrButton = ocr; bar.stack.addArrangedSubview(ocr)
        bar.divider()
        bar.stack.addArrangedSubview(IconButton(icon: "undo", tip: "撤销（⌘Z）") { [weak self] in self?.undo() })
        bar.stack.addArrangedSubview(IconButton(icon: "redo", tip: "重做（⇧⌘Z）") { [weak self] in self?.redo() })
        bar.divider()
        bar.stack.addArrangedSubview(IconButton(icon: "long", tip: "长截屏：滚动页面，自动接成一张长图") { [weak self] in self?.startLong() })
        bar.divider()
        bar.stack.addArrangedSubview(IconButton(icon: "save", tip: "保存为文件") { [weak self] in self?.finish(save: true) })
        let cancel = IconButton(icon: "cancel", tip: "取消（Esc）") { [weak self] in self?.session?.end() }
        cancel.baseTint = NSColor(srgbRed: 0.83, green: 0.22, blue: 0.17, alpha: 1); cancel.fill = nil
        bar.stack.addArrangedSubview(cancel)
        let done = IconButton(icon: "done", tip: "完成，复制到剪贴板（回车）") { [weak self] in self?.finish(save: false) }
        done.fill = ywGold
        bar.stack.addArrangedSubview(done)

        for (i, s) in sizes.enumerated() {
            let d = DotButton(color: NSColor(white: 0.25, alpha: 1), dotSize: 4 + CGFloat(i) * 4) { [weak self] in self?.lineWidth = s; self?.syncBars() }
            sizeDots.append(d); styleBar.stack.addArrangedSubview(d)
        }
        styleBar.divider()
        for c in palette {
            let d = DotButton(color: c, dotSize: 14) { [weak self] in self?.color = c; self?.syncBars() }
            colorDots.append(d); styleBar.stack.addArrangedSubview(d)
        }
        ocrLabel.font = .systemFont(ofSize: 12); ocrLabel.textColor = NSColor(white: 0.4, alpha: 1)
        // 宽度定死（够放“已选 8888 个字”）：拖着选字时字数在变，小条不跟着变宽变窄
        ocrLabel.widthAnchor.constraint(equalToConstant: ceil(("已选 8888 个字" as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12)]).width) + 4).isActive = true
        ocrBar.stack.spacing = 6; ocrBar.stack.edgeInsets = NSEdgeInsets(top: 4, left: 10, bottom: 4, right: 6)
        ocrBar.stack.addArrangedSubview(ocrLabel)
        ocrBar.stack.addArrangedSubview(TextButton("全选", dark: false) { [weak self] in self?.pickAll() })
        ocrBar.stack.addArrangedSubview(TextButton("复制", dark: true) { [weak self] in self?.copyPicked() })
        bar.isHidden = true; styleBar.isHidden = true; tipView.isHidden = true; ocrBar.isHidden = true
        addSubview(bar); addSubview(styleBar); addSubview(ocrBar); addSubview(tipView)
        syncBars()
    }

    private func syncBars() {
        for (t, b) in toolButtons { b.isOn = (t == tool) }
        ocrButton?.isOn = ocrOn
        for (i, d) in sizeDots.enumerated() { d.isOn = sizes[i] == lineWidth }
        for (i, d) in colorDots.enumerated() { d.isOn = palette[i] == color }
        layoutBars()
    }

    func layoutBars() {
        if case .picking = drag { return }                    // 拖着选字时工具条不动
        guard let s = sel, case .none = drag else { bar.isHidden = true; styleBar.isHidden = true; ocrBar.isHidden = true; return }
        let bs = bar.fittingSize
        var x = min(max(4, s.maxX - bs.width), bounds.maxX - bs.width - 4)
        var y = s.minY - bs.height - 8
        var below = true
        if y < 4 { y = s.maxY + 8; below = false }
        if y + bs.height > bounds.maxY - 4 { y = s.minY + 8; below = false; x = min(x, s.maxX - bs.width - 8) }
        bar.frame = NSRect(x: x, y: y, width: bs.width, height: bs.height)
        bar.isHidden = false
        let showStyle = tool != nil && tool != .mosaic
        styleBar.isHidden = !showStyle
        if showStyle {
            let ss = styleBar.fittingSize
            var sy = below ? y - ss.height - 6 : y + bs.height + 6
            if sy < 4 { sy = y + bs.height + 6 }
            if sy + ss.height > bounds.maxY - 4 { sy = y - ss.height - 6 }
            styleBar.frame = NSRect(x: min(max(4, x + bs.width - ss.width), bounds.maxX - ss.width - 4), y: sy, width: ss.width, height: ss.height)
        }
        // “全选、复制”那一条：放在工具条左边；左边放不下就放到工具条外侧那一行
        ocrBar.isHidden = !ocrOn || glyphs.isEmpty
        if !ocrBar.isHidden {
            let os = ocrBar.fittingSize
            var ox = x - os.width - 6, oy = y + (bs.height - os.height) / 2
            if ox < 4 {
                ox = min(max(4, x + bs.width - os.width), bounds.maxX - os.width - 4)
                oy = below ? y - os.height - 6 : y + bs.height + 6
                if oy < 4 { oy = y + bs.height + 6 }
                if oy + os.height > bounds.maxY - 4 { oy = y - os.height - 6 }
            }
            ocrBar.frame = NSRect(x: ox, y: oy, width: os.width, height: os.height)
        }
    }

    // MARK: 绘制

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.draw(image, in: bounds)
        ctx.setFillColor(CGColor(gray: 0, alpha: 0.42)); ctx.fill(bounds)
        guard let s = sel else {
            if active, let h = hover {                    // 鼠标下面的窗口：亮出来，描个边
                ctx.saveGState(); ctx.clip(to: h); ctx.draw(image, in: bounds); ctx.restoreGState()
                ywGold.setStroke(); let b = NSBezierPath(rect: h.insetBy(dx: 1, dy: 1)); b.lineWidth = 2; b.stroke()
            }
            if active { drawHint("拖动鼠标框选，或点一下选中整个窗口；按 Esc 取消") }
            return
        }
        ctx.saveGState(); ctx.clip(to: s)
        ctx.draw(image, in: bounds)
        drawMarks(ctx)
        if ocrOn { drawPicked(ctx) }
        ctx.restoreGState()
        if let t = toast { drawToast(t, in: s) }
        ywGold.setStroke()
        let border = NSBezierPath(rect: s); border.lineWidth = 1.5; border.stroke()
        for p in (ocrOn ? [] : handlePoints(s)) {                 // 提取文字时框不能改大小，控制点不画
            let r = NSRect(x: p.x - 4.5, y: p.y - 4.5, width: 9, height: 9)
            NSColor.white.setFill(); NSBezierPath(ovalIn: r).fill()
            let o = NSBezierPath(ovalIn: r); o.lineWidth = 1.5; o.stroke()
        }
        let label = "\(Int(s.width)) × \(Int(s.height))" as NSString
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: NSColor.white]
        let ls = label.size(withAttributes: attrs)
        var lr = NSRect(x: s.minX, y: s.maxY + 5, width: ls.width + 12, height: ls.height + 4)
        if lr.maxY > bounds.maxY { lr.origin.y = s.maxY - lr.height - 5 }
        NSColor(white: 0.07, alpha: 0.9).setFill(); NSBezierPath(roundedRect: lr, xRadius: 3, yRadius: 3).fill()
        label.draw(at: NSPoint(x: lr.minX + 6, y: lr.minY + 2), withAttributes: attrs)
    }

    private func drawHint(_ text: String) {
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 15, weight: .medium), .foregroundColor: NSColor.white]
        let size = (text as NSString).size(withAttributes: attrs)
        let r = NSRect(x: bounds.midX - size.width / 2 - 14, y: bounds.maxY - 90, width: size.width + 28, height: size.height + 14)
        NSColor(white: 0.07, alpha: 0.82).setFill(); NSBezierPath(roundedRect: r, xRadius: 8, yRadius: 8).fill()
        (text as NSString).draw(at: NSPoint(x: r.minX + 14, y: r.minY + 7), withAttributes: attrs)
    }

    private func pixelated(_ r: NSRect) -> CGImage? {
        let sx = CGFloat(image.width) / bounds.width, sy = CGFloat(image.height) / bounds.height
        let crop = CGRect(x: r.minX * sx, y: (bounds.height - r.maxY) * sy, width: r.width * sx, height: r.height * sy).integral
        guard crop.width > 2, crop.height > 2, let part = image.cropping(to: crop) else { return nil }
        let w = max(1, Int(r.width / 9)), h = max(1, Int(r.height / 9))
        guard let small = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        small.interpolationQuality = .low
        small.draw(part, in: CGRect(x: 0, y: 0, width: w, height: h))
        return small.makeImage()
    }

    func drawMarks(_ ctx: CGContext) {
        for m in marks {
            guard let a = m.points.first, let b = m.points.last else { continue }
            let r = NSRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(a.x - b.x), height: abs(a.y - b.y))
            ctx.setStrokeColor(m.color.cgColor); ctx.setFillColor(m.color.cgColor)
            ctx.setLineWidth(m.width); ctx.setLineCap(.round); ctx.setLineJoin(.round)
            switch m.tool {
            case .text:
                let attr = NSAttributedString(string: m.text, attributes: [.font: NSFont.systemFont(ofSize: m.width, weight: .medium), .foregroundColor: m.color])
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
                attr.draw(at: NSPoint(x: a.x + 2, y: a.y - attr.size().height))
                NSGraphicsContext.restoreGraphicsState()
            case .rect: ctx.stroke(r)
            case .ellipse: ctx.strokeEllipse(in: r)
            case .pen:
                guard m.points.count > 1 else { continue }
                ctx.beginPath(); ctx.move(to: m.points[0]); for p in m.points.dropFirst() { ctx.addLine(to: p) }; ctx.strokePath()
            case .arrow:
                let len = hypot(b.x - a.x, b.y - a.y); guard len > 2 else { continue }
                let head = max(12, m.width * 4.5), ux = (b.x - a.x) / len, uy = (b.y - a.y) / len
                let base = CGPoint(x: b.x - ux * head, y: b.y - uy * head)
                ctx.beginPath(); ctx.move(to: a); ctx.addLine(to: base); ctx.strokePath()
                ctx.beginPath(); ctx.move(to: b)
                ctx.addLine(to: CGPoint(x: base.x - uy * head * 0.5, y: base.y + ux * head * 0.5))
                ctx.addLine(to: CGPoint(x: base.x + uy * head * 0.5, y: base.y - ux * head * 0.5))
                ctx.closePath(); ctx.fillPath()
            case .mosaic:
                if let px = pixelated(r) { ctx.saveGState(); ctx.interpolationQuality = .none; ctx.draw(px, in: r); ctx.restoreGState() }
            }
        }
    }

    // MARK: 鼠标

    private func handlePoints(_ s: NSRect) -> [NSPoint] {
        [NSPoint(x: s.minX, y: s.maxY), NSPoint(x: s.midX, y: s.maxY), NSPoint(x: s.maxX, y: s.maxY),
         NSPoint(x: s.minX, y: s.midY), NSPoint(x: s.maxX, y: s.midY),
         NSPoint(x: s.minX, y: s.minY), NSPoint(x: s.midX, y: s.minY), NSPoint(x: s.maxX, y: s.minY)]
    }
    private let handleDirs: [(Int, Int)] = [(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (-1, -1), (0, -1), (1, -1)]

    private func clamp(_ p: NSPoint) -> NSPoint { NSPoint(x: min(max(p.x, 0), bounds.maxX), y: min(max(p.y, 0), bounds.maxY)) }

    override func mouseDown(with event: NSEvent) {
        guard active else { return }
        commitText()                                    // 点别处：正在打的字先落下来
        let p = clamp(convert(event.locationInWindow, from: nil))
        guard let s = sel else { session?.claim(self); drag = .selecting(p); return }
        if ocrOn {                                      // 提取文字时：在框里按下是选字，别的都不动
            guard s.contains(p), let i = glyphIndex(at: p) else { return }
            if event.clickCount >= 2 {                  // 双击选一行（同一排的字）；要点在字上
                let row = glyphs[i].g.row
                if glyphs[i].rect.insetBy(dx: -4, dy: -4).contains(p), let a = glyphs.firstIndex(where: { $0.g.row == row }), let b = glyphs.lastIndex(where: { $0.g.row == row }) { picked = (a, b) }
            } else { picked = nil; pressAt = p; drag = .picking }   // 单击：取消选中。拖出去一小段才开始选（见 mouseDragged）
            pickChanged(); return
        }
        if let i = handlePoints(s).firstIndex(where: { hypot($0.x - p.x, $0.y - p.y) <= 9 }) { drag = .resizing(i, s); layoutBars(); updateCursor(p); return }
        if s.contains(p) {
            if event.clickCount == 2 && tool == nil { finish(save: false); return }
            if tool == .text { beginText(at: p); layoutBars(); updateCursor(p); return }
            if let t = tool {
                redoStack.removeAll()
                marks.append(Mark(tool: t, points: [p, p], color: color, width: lineWidth))
                drag = .drawing
            } else { drag = .moving(p, s) }
            layoutBars()
        } else if marks.isEmpty {
            sel = nil; drag = .selecting(p); layoutBars(); needsDisplay = true
        }
        updateCursor(p)
    }

    override func mouseDragged(with event: NSEvent) {
        let p = clamp(convert(event.locationInWindow, from: nil))
        switch drag {
        case .selecting(let a):
            sel = NSRect(x: min(a.x, p.x), y: min(a.y, p.y), width: abs(a.x - p.x), height: abs(a.y - p.y))
        case .moving(let start, let orig):
            var r = orig.offsetBy(dx: p.x - start.x, dy: p.y - start.y)
            r.origin.x = min(max(0, r.origin.x), bounds.maxX - r.width); r.origin.y = min(max(0, r.origin.y), bounds.maxY - r.height)
            let dx = r.minX - (sel?.minX ?? r.minX), dy = r.minY - (sel?.minY ?? r.minY)
            for i in marks.indices { marks[i].points = marks[i].points.map { NSPoint(x: $0.x + dx, y: $0.y + dy) } }
            for i in redoStack.indices { redoStack[i].points = redoStack[i].points.map { NSPoint(x: $0.x + dx, y: $0.y + dy) } }
            sel = r
        case .resizing(let i, let orig):
            var minX = orig.minX, maxX = orig.maxX, minY = orig.minY, maxY = orig.maxY
            let (dx, dy) = handleDirs[i]
            if dx < 0 { minX = min(p.x, maxX - 12) }; if dx > 0 { maxX = max(p.x, minX + 12) }
            if dy < 0 { minY = min(p.y, maxY - 12) }; if dy > 0 { maxY = max(p.y, minY + 12) }
            sel = NSRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY)
        case .drawing:
            guard !marks.isEmpty, let s = sel else { break }
            let q = NSPoint(x: min(max(p.x, s.minX), s.maxX), y: min(max(p.y, s.minY), s.maxY))
            if marks[marks.count - 1].tool == .pen { marks[marks.count - 1].points.append(q) } else { marks[marks.count - 1].points[1] = q }
        case .picking:
            // 手抖的一两个点不算；从按下的地方选到现在的地方
            if picked == nil, hypot(p.x - pressAt.x, p.y - pressAt.y) < 3 { break }
            if let a = picked?.anchor ?? glyphIndex(at: pressAt), let i = glyphIndex(at: p) { picked = (a, i); pickChanged() }
        case .none: break
        }
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        if case .selecting = drag, let s = sel, s.width < 4 || s.height < 4 { sel = nil }
        if case .selecting = drag, sel == nil {
            // 没拖动、直接点：选中点的那个位置下面的窗口（按点击点现找，不靠之前鼠标移动时记的）
            let p = convert(event.locationInWindow, from: nil)
            if let h = pick(at: p, fresh: true) { sel = h; hover = nil }
            else { session?.release() }
        }
        if case .drawing = drag, let m = marks.last, let a = m.points.first, let b = m.points.last, m.points.count <= 2, hypot(a.x - b.x, a.y - b.y) < 2 { marks.removeLast() }
        drag = .none
        layoutBars(); needsDisplay = true
        updateCursor(convert(event.locationInWindow, from: nil))
    }

    override func keyDown(with event: NSEvent) {
        let cmd = event.modifierFlags.contains(.command)
        switch event.keyCode {
        case 53: session?.end()
        case 36, 76: if sel != nil { finish(save: false) }
        case 6 where cmd: if event.modifierFlags.contains(.shift) { redo() } else { undo() }
        default:
            // ⌘A、⌘C 认的是键上印的字母（换了键盘布局也对）
            guard cmd, ocrOn else { break }
            switch event.charactersIgnoringModifiers?.lowercased() { case "a": pickAll(); case "c": copyPicked(); default: break }
        }
    }

    // 提取文字时标注工具停用，撤销重做也不动标注
    func undo() { if !dragging, !ocrOn, let m = marks.popLast() { redoStack.append(m); needsDisplay = true } }
    func redo() { if !dragging, !ocrOn, let m = redoStack.popLast() { marks.append(m); needsDisplay = true } }

    // MARK: 输出

    func exportImage() -> CGImage? {
        guard let s = sel else { return nil }
        // 选区换算成整数像素后直接从原图裁，原图像素一个不动；标注再画在上面
        let sx = CGFloat(image.width) / bounds.width, sy = CGFloat(image.height) / bounds.height
        let x0 = (s.minX * sx).rounded(), x1 = (s.maxX * sx).rounded()
        let y0 = ((bounds.height - s.maxY) * sy).rounded(), y1 = ((bounds.height - s.minY) * sy).rounded()
        let w = Int(x1 - x0), h = Int(y1 - y0)
        guard w > 0, h > 0, let part = image.cropping(to: CGRect(x: x0, y: y0, width: CGFloat(w), height: CGFloat(h))),
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.draw(part, in: CGRect(x: 0, y: 0, width: w, height: h))
        ctx.scaleBy(x: sx, y: sy)
        ctx.translateBy(x: -x0 / sx, y: -(bounds.height - y1 / sy))
        drawMarks(ctx)
        return ctx.makeImage()
    }

    func finish(save: Bool) {
        commitText()
        guard let s = sel, let cg = exportImage() else { session?.end(); return }
        deliverShot(cg, pointSize: s.size, save: save, dir: session?.saveDir ?? "~/Desktop")
        session?.end()
    }

    // MARK: 点选

    /// 这个位置能点选的东西：最上面那个窗口；如果上面盖着通知中心、程序坞这种系统层，先看它里面有没有东西在这个位置。
    private func pick(at p: NSPoint, fresh: Bool) -> NSRect? {
        for t in targets where t.rect.contains(p) {
            if !t.system { return t.rect }
            if let r = elementRect(pid: t.pid, at: p, fresh: fresh) { return r }
        }
        return nil
    }

    /// 向系统问：这个应用在这个位置的界面元素是哪一块（比如一条通知横幅）。需要“辅助功能”权限，没有就算了。
    private func elementRect(pid: pid_t, at p: NSPoint, fresh: Bool) -> NSRect? {
        guard AXIsProcessTrusted() else { return nil }
        let now = ProcessInfo.processInfo.systemUptime
        if !fresh, let c = axCache, c.pid == pid, now - c.at < 0.08 { return (c.rect?.contains(p) ?? false) ? c.rect : nil }   // 鼠标移动时别问得太勤
        let primaryH = NSScreen.screens.first?.frame.height ?? screenFrame.height
        let gx = screenFrame.minX + p.x, gy = primaryH - (screenFrame.minY + p.y)
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 0.1)
        var hit: AXUIElement?
        var found: NSRect?
        if AXUIElementCopyElementAtPosition(app, Float(gx), Float(gy), &hit) == .success, var e = hit {
            let limit = screenFrame.width * screenFrame.height * 0.5
            for _ in 0..<12 {                                    // 一层层往外找，取“不到半个屏幕”的最大那一块
                var pos: CFTypeRef?, size: CFTypeRef?
                if AXUIElementCopyAttributeValue(e, kAXPositionAttribute as CFString, &pos) == .success,
                   AXUIElementCopyAttributeValue(e, kAXSizeAttribute as CFString, &size) == .success,
                   let pv = pos, let sv = size, CFGetTypeID(pv) == AXValueGetTypeID(), CFGetTypeID(sv) == AXValueGetTypeID() {
                    var o = CGPoint.zero, z = CGSize.zero
                    AXValueGetValue(pv as! AXValue, .cgPoint, &o); AXValueGetValue(sv as! AXValue, .cgSize, &z)
                    if z.width >= 30, z.height >= 20, z.width * z.height < limit {
                        let local = NSRect(x: o.x - screenFrame.minX, y: primaryH - o.y - z.height - screenFrame.minY, width: z.width, height: z.height)
                            .intersection(NSRect(origin: .zero, size: screenFrame.size))
                        if local.contains(p) { found = local }
                    }
                }
                var parent: CFTypeRef?
                guard AXUIElementCopyAttributeValue(e, kAXParentAttribute as CFString, &parent) == .success, let pe = parent, CFGetTypeID(pe) == AXUIElementGetTypeID() else { break }
                e = pe as! AXUIElement
            }
        }
        axCache = (now, pid, found)
        return found
    }

    /// 鼠标停在工具条按钮上片刻，在工具条旁边显示它的名字；传 nil 收起。
    func showTip(_ button: IconButton?) {
        tipToken += 1
        tipView.isHidden = true
        guard let b = button, !b.tip.isEmpty else { return }
        let token = tipToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self, weak b] in
            guard let self = self, let b = b, self.tipToken == token, let barView = b.superview?.superview else { return }
            self.tipView.text = b.tip
            let size = self.tipView.fitting, at = b.convert(b.bounds, to: self), barFrame = barView.frame
            // 放在工具条外侧：另一条工具条在上面就放下面，否则放上面
            let other = barView === self.bar ? self.styleBar : self.bar
            var y = (!other.isHidden && other.frame.minY > barFrame.minY) ? barFrame.minY - size.height - 6 : barFrame.maxY + 6
            if y + size.height > self.bounds.maxY - 2 { y = barFrame.minY - size.height - 6 }
            if y < 2 { y = barFrame.maxY + 6 }
            let x = min(max(4, at.midX - size.width / 2), self.bounds.maxX - size.width - 4)
            self.tipView.frame = NSRect(x: x, y: y, width: size.width, height: size.height)
            self.tipView.isHidden = false
        }
    }

    func startLong() {
        cancelText(); ocrOff()
        guard let s = sel, let screen = window?.screen else { return }
        session?.startLong(screen: screen, rect: s)
    }

    // MARK: 提取文字

    /// 框里的原图（不带标注），认字用
    private func plainCrop() -> CGImage? {
        guard let s = sel else { return nil }
        let sx = CGFloat(image.width) / bounds.width, sy = CGFloat(image.height) / bounds.height
        return image.cropping(to: CGRect(x: s.minX * sx, y: (bounds.height - s.maxY) * sy, width: s.width * sx, height: s.height * sy).integral)
    }
    private func ocrStart() {
        commitText()
        guard let s = sel, let crop = plainCrop() else { return }
        tool = nil
        ocrOn = true; glyphs = []; picked = nil
        ocrToken += 1
        let token = ocrToken
        say("正在识别…", for: 60)
        syncBars(); needsDisplay = true
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let found = TextFinder.find(in: crop)
            DispatchQueue.main.async {
                guard let self, self.ocrOn, self.ocrToken == token, self.sel == s else { return }
                if found.isEmpty { self.ocrOff(); self.say("没有识别到文字", for: 1.4); return }
                self.glyphs = OverlayView.place(found, in: s)
                self.say(nil, for: 0)
                self.pickChanged()
                if let w = self.window { self.updateCursor(self.convert(w.mouseLocationOutsideOfEventStream, from: nil)) }
            }
        }
    }
    /// 认出来的字从“在图里的比例”换成视图里的位置
    private static func place(_ found: [OCRGlyph], in s: NSRect) -> [(g: OCRGlyph, rect: NSRect)] {
        found.map { g in (g, NSRect(x: s.minX + g.box.minX * s.width, y: s.minY + g.box.minY * s.height, width: g.box.width * s.width, height: g.box.height * s.height)) }
    }
    /// 出检查用的截图时用：当场认字（不等后台），选中其中一段
    func ocrForSnapshot(pick range: ClosedRange<Int>) {
        guard let s = sel, let crop = plainCrop() else { return }
        tool = nil; ocrOn = true
        glyphs = OverlayView.place(TextFinder.find(in: crop), in: s)
        picked = glyphs.count > range.upperBound ? (range.lowerBound, range.upperBound) : nil
        syncBars(); pickChanged()
    }
    private func ocrOff() {
        guard ocrOn else { return }
        ocrOn = false; ocrToken += 1; glyphs = []; picked = nil
        if case .picking = drag { drag = .none }
        say(nil, for: 0)
        syncBars(); needsDisplay = true
        if let w = window { updateCursor(convert(w.mouseLocationOutsideOfEventStream, from: nil)) }
    }
    /// 这个位置算哪个字：先找上下最近的那一排，再在这一排里找左右最近的
    private func glyphIndex(at p: NSPoint) -> Int? {
        guard !glyphs.isEmpty else { return nil }
        let dy: (NSRect) -> CGFloat = { p.y < $0.minY ? $0.minY - p.y : (p.y > $0.maxY ? p.y - $0.maxY : 0) }
        let dx: (NSRect) -> CGFloat = { p.x < $0.minX ? $0.minX - p.x : (p.x > $0.maxX ? p.x - $0.maxX : 0) }
        guard let row = glyphs.min(by: { dy($0.rect) < dy($1.rect) })?.g.row else { return nil }
        return glyphs.indices.filter { glyphs[$0].g.row == row }.min(by: { dx(glyphs[$0].rect) < dx(glyphs[$1].rect) })
    }
    private var pickedRange: ClosedRange<Int>? { picked.map { min($0.anchor, $0.focus)...max($0.anchor, $0.focus) } }
    private func pickChanged() {
        let n = pickedRange.map { glyphs[$0].filter { !$0.g.ch.isWhitespace }.count } ?? 0
        ocrLabel.stringValue = n > 0 ? "已选 \(n) 个字" : "拖动选字"
        layoutBars(); needsDisplay = true
    }
    private func pickAll() { guard ocrOn, !glyphs.isEmpty else { return }; picked = (0, glyphs.count - 1); pickChanged() }
    /// 复制选中的字；什么都没选就复制全部。截图不关，可以接着选别的
    private func copyPicked() {
        guard ocrOn, !glyphs.isEmpty else { return }
        let range = pickedRange ?? 0...(glyphs.count - 1)
        let text = TextFinder.text(of: glyphs[range].map(\.g)[...])
        let pb = NSPasteboard.general
        pb.clearContents(); pb.setString(text, forType: .string)
        say("已复制 \(text.filter { !$0.isWhitespace }.count) 个字", for: 1.2)
    }
    /// 认出来的字垫一层淡蓝，选中的深一些。一段里连着的并成一条，看着是整行
    private func drawPicked(_ ctx: CGContext) {
        func fill(_ idx: [Int], _ color: CGColor) {
            var run: NSRect?
            var last = -2, seg = -1
            ctx.setFillColor(color)
            for i in idx {
                let g = glyphs[i]
                if i == last + 1, g.g.seg == seg, let r = run { run = r.union(g.rect) }
                else { if let r = run { ctx.fill(r.insetBy(dx: -1, dy: -1)) }; run = g.rect }
                last = i; seg = g.g.seg
            }
            if let r = run { ctx.fill(r.insetBy(dx: -1, dy: -1)) }
        }
        fill(Array(glyphs.indices), CGColor(srgbRed: 0.20, green: 0.47, blue: 0.96, alpha: 0.12))
        if let r = pickedRange { fill(Array(r), CGColor(srgbRed: 0.20, green: 0.47, blue: 0.96, alpha: 0.36)) }
    }
    /// 框中间的一句提示（正在识别、已复制…）；seconds 之后自己消失。传 nil 马上收起
    private func say(_ text: String?, for seconds: TimeInterval) {
        toast = text; toastToken += 1; needsDisplay = true
        guard text != nil else { return }
        let token = toastToken
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [weak self] in
            guard let self, self.toastToken == token else { return }
            self.toast = nil; self.needsDisplay = true
        }
    }
    private func drawToast(_ text: String, in s: NSRect) {
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 13, weight: .medium), .foregroundColor: NSColor.white]
        let size = (text as NSString).size(withAttributes: attrs)
        let r = NSRect(x: s.midX - size.width / 2 - 14, y: s.midY - size.height / 2 - 6, width: size.width + 28, height: size.height + 12)
        NSColor(white: 0.08, alpha: 0.88).setFill(); NSBezierPath(roundedRect: r, xRadius: 8, yRadius: 8).fill()
        (text as NSString).draw(at: NSPoint(x: r.minX + 14, y: r.minY + 6), withAttributes: attrs)
    }

    // MARK: 文字标注

    private var fontSize: CGFloat { lineWidth <= 2 ? 14 : (lineWidth <= 3 ? 18 : 28) }

    private func beginText(at p: NSPoint) {
        commitText()
        let f = NSTextField(frame: .zero)
        f.isBordered = false; f.drawsBackground = false; f.focusRingType = .none; f.isBezeled = false
        f.font = .systemFont(ofSize: fontSize, weight: .medium); f.textColor = color
        f.cell?.wraps = false; f.cell?.isScrollable = true
        f.delegate = self
        editor = f; editorTop = p
        addSubview(f)
        fitEditor()
        window?.makeFirstResponder(f)
    }
    private func fitEditor() {
        guard let f = editor else { return }
        let text = f.stringValue.isEmpty ? "字" : f.stringValue
        let size = (text as NSString).size(withAttributes: [.font: f.font ?? NSFont.systemFont(ofSize: fontSize)])
        let w = min(size.width + fontSize * 2, max(40, bounds.maxX - editorTop.x - 4))
        f.frame = NSRect(x: editorTop.x, y: editorTop.y - size.height, width: w, height: size.height)
    }
    func commitText() {
        guard let f = editor else { return }
        editor = nil                                   // 先清掉，免得结束编辑的通知又绕回来
        let text = f.stringValue, c = f.textColor ?? color, size = f.font?.pointSize ?? fontSize
        f.removeFromSuperview()
        if !text.isEmpty {
            redoStack.removeAll()
            marks.append(Mark(tool: .text, points: [editorTop], color: c, width: size, text: text))
        }
        window?.makeFirstResponder(self)
        needsDisplay = true
    }
    /// Esc：输入法正在拼字时只丢掉没拼完的那段，否则取消这段文字。
    func escapeText() {
        if let tv = editor?.currentEditor() as? NSTextView, tv.hasMarkedText() {
            tv.inputContext?.discardMarkedText()
            tv.setMarkedText("", selectedRange: NSRange(location: 0, length: 0), replacementRange: tv.markedRange())
            fitEditor()
            return
        }
        cancelText()
    }
    func cancelText() {
        guard let f = editor else { return }
        editor = nil
        f.removeFromSuperview()
        window?.makeFirstResponder(self)
        needsDisplay = true
    }
}

extension OverlayView: NSTextFieldDelegate {
    func controlTextDidChange(_ obj: Notification) { fitEditor() }
    func controlTextDidEndEditing(_ obj: Notification) { commitText() }
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.cancelOperation(_:)) { cancelText(); return true }
        return false
    }
}
