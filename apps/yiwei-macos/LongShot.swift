// 一苇 · 长截屏：选区不动，用户滚动页面，把每次新露出来的内容接在下面，最后得到一张长图
import Cocoa
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

/// 拼接：比较相邻两帧，找出内容往上走了多少行，把新露出来的那几行接上去。只认向下滚动。
/// 页面顶部、底部固定不动的栏（标题栏、输入框）会被认出来：比对时避开，底栏只在最后补一次。
final class Stitcher {
    /// first 第一帧；same 没动；added 往下滚了、接上了几行；above 往回滚到了断点上方（还对得上，继续往下滚就行）；
    /// lost 对不上；full 到最长了
    enum Step { case first, same, added(Int), above, lost, full }

    // 省内存：接好的每一段都压成 PNG 存着（截图压缩后只有原来的几分之一到十几分之一），
    // 另留一张很小的缩略图给预览用；到最后“完成”时才一次解开拼成整图。
    private var parts: [(data: Data, height: Int)] = []
    private(set) var thumbs: [CGImage] = []
    private(set) var width = 0                   // 长图的宽度（只接会滚动的那一栏时，比选区窄）
    private var bodyHeight = 0                   // 已接好的高度，不含底栏
    private var footer: (data: Data, height: Int)?
    private(set) var footerThumb: CGImage?
    private var prev: [UInt8] = []               // 上一帧的“行特征”：每行横向压成 cols 个灰度值
    private var frameWidth = 0, frameHeight = 0
    private var top = -1, bottom = -1            // 顶部、底部固定不动的行数；第一次接上时定下来
    // 选区里真正在滚动的那一栏（像素列）。框选整个窗口时，侧栏不动、只有中间在滚，只能拿中间这一栏来比、来接。
    private var region: Range<Int>?
    private var firstFrame: CGImage?             // 这一栏定下来之前，留着第一帧
    private var firstGray: [UInt8] = []
    private let cols = 16
    let maxHeight = 20000                        // 长图最多这么高（像素），再长就停
    var totalHeight: Int { bodyHeight + (footer?.height ?? 0) }
    var storedBytes: Int { parts.reduce(0) { $0 + $1.data.count } + (footer?.data.count ?? 0) }
    var narrowed: Bool { region.map { $0.count < frameWidth } ?? false }

    private func png(_ img: CGImage) -> Data? {
        let d = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(d, UTType.png.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dest, img, nil)
        return CGImageDestinationFinalize(dest) ? d as Data : nil
    }
    private func unpack(_ d: Data) -> CGImage? {
        guard let src = CGImageSourceCreateWithData(d as CFData, nil) else { return nil }
        return CGImageSourceCreateImageAtIndex(src, 0, nil)
    }
    private func thumb(_ img: CGImage) -> CGImage? {
        let w = min(336, img.width), h = max(1, img.height * w / img.width)
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .medium
        ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
        return ctx.makeImage()
    }
    // 全程沿用屏幕画面自己的色彩空间，存、取、拼都不做颜色换算
    private var colorSpace: CGColorSpace?
    private func space(_ img: CGImage) -> CGColorSpace {
        if colorSpace == nil { colorSpace = (img.colorSpace?.model == .rgb ? img.colorSpace : nil) ?? CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB() }
        return colorSpace!
    }

    /// 每行横向压成 cols 个灰度值
    private func signature(_ img: CGImage, _ columns: Range<Int>) -> [UInt8]? {
        let h = img.height
        guard let src = columns.count == img.width ? img : img.cropping(to: CGRect(x: columns.lowerBound, y: 0, width: columns.count, height: h)) else { return nil }
        var buf = [UInt8](repeating: 0, count: cols * h)
        let ok = buf.withUnsafeMutableBytes { raw -> Bool in
            guard let ctx = CGContext(data: raw.baseAddress, width: cols, height: h, bitsPerComponent: 8, bytesPerRow: cols,
                                      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return false }
            ctx.interpolationQuality = .medium
            ctx.draw(src, in: CGRect(x: 0, y: 0, width: cols, height: h))
            return true
        }
        return ok ? buf : nil
    }
    /// 整帧的灰度图（原大小），用来找“哪几列在动”
    private func gray(_ img: CGImage) -> [UInt8]? {
        let w = img.width, h = img.height
        var buf = [UInt8](repeating: 0, count: w * h)
        let ok = buf.withUnsafeMutableBytes { raw -> Bool in
            guard let ctx = CGContext(data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w,
                                      space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return false }
            ctx.interpolationQuality = .none
            ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        return ok ? buf : nil
    }

    /// 把图里指定的列、从上往下数的第 rows 行抄成一张独立的小图（直接裁出来的图会一直占着整帧的内存）
    private func copy(_ img: CGImage, columns: Range<Int>, rows: Range<Int>) -> CGImage? {
        guard !rows.isEmpty, !columns.isEmpty,
              let part = img.cropping(to: CGRect(x: columns.lowerBound, y: rows.lowerBound, width: columns.count, height: rows.count)),
              let ctx = CGContext(data: nil, width: columns.count, height: rows.count, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: space(img), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .none
        ctx.draw(part, in: CGRect(x: 0, y: 0, width: columns.count, height: rows.count))
        return ctx.makeImage()
    }

    /// 比较两帧，找出在动的那几列。没有变化返回 nil。
    private func scrollColumns(_ a: [UInt8], _ b: [UInt8], w: Int, h: Int) -> Range<Int>? {
        let r0 = h / 10, r1 = h - h / 10
        var first = -1, last = -1
        a.withUnsafeBufferPointer { pa in
            b.withUnsafeBufferPointer { pb in
                for c in 0..<w {
                    var sum = 0, n = 0, r = r0
                    while r < r1 { sum += abs(Int(pa[r * w + c]) - Int(pb[r * w + c])); n += 1; r += 3 }
                    if sum > n * 2 { if first < 0 { first = c }; last = c }
                }
            }
        }
        guard first >= 0 else { return nil }
        var lo = first, hi = last + 1
        if hi - lo >= w * 9 / 10 { return 0..<w }
        // 往两边带上同样底色的空白边（正文两侧的留白），碰到别的东西（分隔线、侧栏）就停
        func column(_ c: Int) -> (mean: Int, flat: Bool) {
            var mn = 255, mx = 0, sum = 0, n = 0, r = r0
            while r < r1 { let v = Int(b[r * w + c]); mn = min(mn, v); mx = max(mx, v); sum += v; n += 1; r += 3 }
            return (sum / max(1, n), mx - mn <= 6)
        }
        if lo > 0 {
            let edge = column(lo - 1)
            if edge.flat {
                var c = lo - 1; while c >= 0 { let x = column(c); if !x.flat || abs(x.mean - edge.mean) > 6 { break }; c -= 1 }
                if lo - (c + 1) >= 4 { lo = c + 1 }          // 只有一两列宽的是分隔线，不是留白，不带
            }
        }
        if hi < w {
            let edge = column(hi)
            if edge.flat {
                var c = hi; while c < w { let x = column(c); if !x.flat || abs(x.mean - edge.mean) > 6 { break }; c += 1 }
                if c - hi >= 4 { hi = c }
            }
        }
        return lo..<hi
    }

    func add(_ img: CGImage) -> Step {
        let h = img.height
        if parts.isEmpty {
            let all = 0..<img.width
            guard let whole = copy(img, columns: all, rows: 0..<h), let data = png(whole), let small = thumb(whole),
                  let sig = signature(img, all), let g = gray(img) else { return .lost }
            parts = [(data, h)]; thumbs = [small]; width = img.width; bodyHeight = h
            frameWidth = img.width; frameHeight = h; prev = sig; firstFrame = whole; firstGray = g
            return .first
        }
        guard img.width == frameWidth, h == frameHeight else { return .lost }

        // 滚动的那一栏还没定：看这一帧和第一帧比，哪几列在动。先当成候选，这一帧真接上了才算数。
        var use = region ?? 0..<frameWidth
        var old = prev
        if region == nil {
            guard let g = gray(img), let moving = scrollColumns(firstGray, g, w: frameWidth, h: h) else { return .same }
            if moving.count < max(60, frameWidth / 20) { return .same }        // 只是光标闪一下之类的小动静
            use = moving
            if moving.count < frameWidth, let first = firstFrame, let s = signature(first, moving) { old = s }
        }
        guard let sig = signature(img, use) else { return .lost }

        let c0 = 1, c1 = cols - 1                      // 最左最右两列不比：最右常是滚动条
        let per = c1 - c0
        func rowDiff(_ ra: Int, _ rb: Int) -> Int {    // 旧帧第 ra 行和新帧第 rb 行差多少
            var sum = 0
            for c in c0..<c1 { sum += abs(Int(old[ra * cols + c]) - Int(sig[rb * cols + c])) }
            return sum
        }
        // 上下固定不动的栏：从顶、从底数，连续多少行和上一帧同一位置一模一样
        var t = top, f = bottom
        if t < 0 {
            t = 0; while t < h / 4 && rowDiff(t, t) <= per { t += 1 }
            f = 0; while f < h / 4 && rowDiff(h - 1 - f, h - 1 - f) <= per { f += 1 }
        }
        let lo = max(h / 10, t), pad = max(h / 10, f), minOverlap = max(h / 6, 24)
        let maxShift = h - lo - pad - minOverlap
        guard maxShift >= 1 else { return .lost }
        // down 为真：新帧的第 r 行应该等于旧帧的第 r+d 行（往下滚了 d 行）；为假则反过来（往回滚了 d 行）。
        // 返回平均每格差多少（0~255）
        func cost(_ d: Int, down: Bool) -> Double {
            var sum = 0, n = 0
            old.withUnsafeBufferPointer { a in
                sig.withUnsafeBufferPointer { b in
                    var r = lo
                    while r < h - d - pad {
                        let pa = (down ? r + d : r) * cols, pb = (down ? r : r + d) * cols
                        for c in c0..<c1 { sum += abs(Int(a[pa + c]) - Int(b[pb + c])) }
                        n += 1; r += 2
                    }
                }
            }
            return n == 0 ? .infinity : Double(sum) / Double(n * per)
        }
        let still = cost(0, down: true)
        if still < 0.6 { return .same }
        var costs = [Double](repeating: .infinity, count: maxShift + 1)
        var best = Double.infinity
        for d in 1...maxShift { costs[d] = cost(d, down: true); if costs[d] < best { best = costs[d] } }
        guard best < 2.5, best < still * 0.5 else {
            // 往下对不上：看看是不是往回滚到了断点上方
            for d in 1...maxShift { let c = cost(d, down: false); if c < 2.5 && c < still * 0.5 { return .above } }
            return .lost                                // 滚太快，中间漏了一段
        }
        // 表格、列表这类一行行重复的内容会有好几个都对得上的位置：截得够密，取滚得最少的那个
        let shift = (1...maxShift).first(where: { costs[$0] <= best + 0.3 }) ?? 1
        guard totalHeight + shift <= maxHeight else { return .full }

        if region == nil {                              // 这一帧接上了：滚动的那一栏就此定下来
            region = use
            if use.count < frameWidth, let first = firstFrame, let body = copy(first, columns: use, rows: 0..<h), let data = png(body), let small = thumb(body) {
                parts[0] = (data, h); thumbs[0] = small; width = use.count
            } else { region = 0..<frameWidth }
            firstFrame = nil; firstGray = []
        }
        let columns = region ?? 0..<frameWidth
        if top < 0 {
            top = t; bottom = f
            // 第一帧里的底栏先拿掉
            if f > 0, let first = unpack(parts[0].data), let body = copy(first, columns: 0..<first.width, rows: 0..<(h - f)), let data = png(body), let small = thumb(body) {
                parts[0] = (data, h - f); thumbs[0] = small; bodyHeight = h - f
            } else { bottom = 0 }
        }
        guard let strip = copy(img, columns: columns, rows: (h - bottom - shift)..<(h - bottom)), let data = png(strip), let small = thumb(strip) else { return .lost }
        parts.append((data, shift)); thumbs.append(small); bodyHeight += shift; prev = sig
        if bottom > 0, let foot = copy(img, columns: columns, rows: (h - bottom)..<h), let fd = png(foot) { footer = (fd, bottom); footerThumb = thumb(foot) }
        return .added(shift)
    }

    func result() -> CGImage? {
        let total = totalHeight
        guard width > 0, total > 0,
              let ctx = CGContext(data: nil, width: width, height: total, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: colorSpace ?? CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.interpolationQuality = .none
        var y = 0
        for p in parts + (footer.map { [$0] } ?? []) {
            guard let img = unpack(p.data) else { return nil }
            ctx.draw(img, in: CGRect(x: 0, y: total - y - p.height, width: width, height: p.height))
            y += p.height
        }
        return ctx.makeImage()
    }
}

/// 预览：显示长图最下面的一段，新接上的内容从底部冒出来
private final class LongPreview: NSView {
    weak var stitcher: Stitcher?
    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.93, alpha: 1).setFill(); bounds.fill()
        guard let st = stitcher, st.width > 0, let ctx = NSGraphicsContext.current?.cgContext else { return }
        var y: CGFloat = 0
        for s in ((st.footerThumb.map { [$0] } ?? []) + st.thumbs.reversed()) {
            let h = CGFloat(s.height) * bounds.width / CGFloat(max(1, s.width))
            ctx.draw(s, in: CGRect(x: 0, y: y, width: bounds.width, height: h))
            y += h
            if y >= bounds.height { break }
        }
    }
}

/// 选区外压暗、描边；鼠标事件全部穿透给下面的窗口
private final class LongFrameView: NSView {
    var hole = NSRect.zero
    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.setFillColor(CGColor(gray: 0, alpha: 0.35)); ctx.fill(bounds)
        ctx.clear(hole)
        ywGold.setStroke()
        let b = NSBezierPath(rect: hole.insetBy(dx: -1.5, dy: -1.5)); b.lineWidth = 2; b.stroke()
    }
}

final class LongShot {
    private let screen: NSScreen, rect: NSRect, saveDir: String, onEnd: () -> Void
    var previousApp: NSRunningApplication?        // 截图前在最前面的那个应用
    private let stitcher = Stitcher()
    private var frameWin: NSPanel?, panel: NSPanel?
    private let preview = LongPreview()
    private let status = NSTextField(wrappingLabelWithString: "")
    private var timer: Timer?
    private var busy = false, closed = false
    // 自动滚动和回到断点都要一苇替你滚页面，这需要系统的“辅助功能”权限（第一次用时系统会问）
    private enum Mode { case manual, auto, seeking }
    private var mode = Mode.manual
    private var down: Int32 = -1                  // 往下滚对应的滚轮方向；发现滚反了会自动换
    private var idle = 0, wrongWay = 0, sought = 0
    private var autoButton: NSButton?, backButton: NSButton?
    private var filter: SCContentFilter?
    private let config = SCStreamConfiguration()

    /// rect 是选区在这块屏幕里的位置（左下角为原点）
    init(screen: NSScreen, rect: NSRect, saveDir: String, onEnd: @escaping () -> Void) {
        // 选区对齐到整点，截出来的每一帧才是一样大、不发虚
        let r = NSRect(x: rect.minX.rounded(), y: rect.minY.rounded(), width: max(20, rect.width.rounded()), height: max(20, rect.height.rounded()))
        self.screen = screen; self.rect = r; self.saveDir = saveDir; self.onEnd = onEnd
    }

    func start() {
        let fw = NSPanel(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        fw.setFrame(screen.frame, display: false)
        fw.level = .screenSaver; fw.isOpaque = false; fw.backgroundColor = .clear; fw.hasShadow = false
        fw.ignoresMouseEvents = true; fw.hidesOnDeactivate = false
        fw.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let fv = LongFrameView(frame: NSRect(origin: .zero, size: screen.frame.size)); fv.hole = rect
        fw.contentView = fv
        fw.orderFrontRegardless()
        frameWin = fw

        // 控制条：预览、提示、取消 / 保存 / 完成。放在选区右边，放不下就放左边，再不行就压在选区里靠右
        let w: CGFloat = 188, previewH = max(120, min(rect.height - 128, 380)), h = previewH + 128
        var x = rect.maxX + 12
        if x + w > screen.frame.width - 4 { x = rect.minX - 12 - w }
        if x < 4 { x = rect.maxX - w - 8 }
        let y = min(max(4, rect.maxY - h), screen.frame.height - h - 4)
        let p = NSPanel(contentRect: NSRect(x: screen.frame.minX + x, y: screen.frame.minY + y, width: w, height: h),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        p.level = NSWindow.Level(rawValue: NSWindow.Level.screenSaver.rawValue + 1)
        p.isOpaque = false; p.backgroundColor = .clear; p.hasShadow = true; p.hidesOnDeactivate = false
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        let box = NSView(frame: NSRect(x: 0, y: 0, width: w, height: h))
        box.wantsLayer = true; box.layer?.backgroundColor = NSColor.white.cgColor; box.layer?.cornerRadius = 10
        preview.stitcher = stitcher
        preview.frame = NSRect(x: 10, y: 118, width: w - 20, height: previewH)
        preview.wantsLayer = true; preview.layer?.cornerRadius = 4; preview.layer?.masksToBounds = true
        status.frame = NSRect(x: 10, y: 72, width: w - 20, height: 40)
        status.font = .systemFont(ofSize: 11.5); status.textColor = NSColor(white: 0.25, alpha: 1)
        status.stringValue = "在框里向下滚动页面，会自动接成长图"
        func button(_ title: String, _ x: CGFloat, _ width: CGFloat, _ action: Selector, gold: Bool = false, row: CGFloat = 8) -> NSButton {
            let b = NSButton(title: title, target: self, action: action)
            b.frame = NSRect(x: x, y: row, width: width, height: 26); b.bezelStyle = .rounded; b.font = .systemFont(ofSize: 12)
            if gold { b.bezelColor = ywGold }
            return b
        }
        box.addSubview(preview); box.addSubview(status)
        box.addSubview(button("取消", 8, 54, #selector(cancelTapped)))
        box.addSubview(button("保存", 64, 54, #selector(saveTapped)))
        box.addSubview(button("完成", 120, 60, #selector(doneTapped), gold: true))
        let auto = button("自动滚动", 8, 86, #selector(autoTapped), row: 40)
        let back = button("回到断点", 94, 86, #selector(backTapped), row: 40)
        back.isHidden = true
        box.addSubview(auto); box.addSubview(back)
        autoButton = auto; backButton = back
        p.contentView = box
        p.orderFrontRegardless()
        panel = p
        if let app = previousApp { app.activate() } else { NSApp.deactivate() }   // 把键盘焦点还给原来的应用，空格、翻页键也能用来滚动

        let scale = screen.backingScaleFactor
        config.sourceRect = CGRect(x: rect.minX, y: screen.frame.height - rect.maxY, width: rect.width, height: rect.height)
        config.width = Int((rect.width * scale).rounded()); config.height = Int((rect.height * scale).rounded())
        config.showsCursor = false
        Task { @MainActor in
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                let id = self.screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID
                guard let display = content.displays.first(where: { $0.displayID == id }) else { self.onEnd(); return }
                // 截的时候把一苇自己的窗口（压暗层、控制条）排除在外
                let mine = content.applications.filter { $0.processID == ProcessInfo.processInfo.processIdentifier }
                self.filter = SCContentFilter(display: display, excludingApplications: mine, exceptingWindows: [])
                guard !self.closed else { return }
                self.timer = Timer.scheduledTimer(withTimeInterval: 0.12, repeats: true) { [weak self] _ in self?.tick() }
            } catch { self.onEnd() }
        }
    }

    private func tick() {
        guard !busy, !closed, let filter = filter else { return }
        busy = true
        Task { @MainActor in
            defer { self.busy = false }
            guard let img = try? await SCScreenshotManager.captureImage(contentFilter: filter, configuration: self.config), !self.closed else { return }
            let step = self.stitcher.add(img)
            let height = Int(CGFloat(self.stitcher.totalHeight) / self.screen.backingScaleFactor)
            var lost = false
            switch step {
            case .first: break
            case .added:
                self.status.stringValue = "已接到 \(height) 点高" + (self.stitcher.narrowed ? "（只接会滚动的那一栏）" : "") + "，滚完点“完成”"
                self.preview.needsDisplay = true
                self.idle = 0; self.wrongWay = 0
            case .same: self.idle += 1
            case .above:
                self.status.stringValue = "已经回到断点上方，继续向下滚就能接上"
                self.wrongWay += 1
            case .lost:
                lost = true
                if self.mode != .seeking { self.status.stringValue = "没对上：滚得太快，中间漏了一段。往回滚一点，或点“回到断点”" }
            case .full:
                self.status.stringValue = "已经到最长了，点“完成”"
                self.setMode(.manual)
            }
            self.backButton?.isHidden = !(lost && self.mode == .manual)
            self.drive(step)
        }
    }

    // MARK: 替用户滚动（自动滚动、回到断点）

    /// 没有“辅助功能”权限就让系统弹出授权提示，并告诉用户怎么做。
    private func canScroll() -> Bool {
        let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        if AXIsProcessTrustedWithOptions(opts) { return true }
        status.stringValue = "要先在系统设置的“辅助功能”里把“一苇”打开，开好后再点一次"
        return false
    }
    private func setMode(_ m: Mode) {
        mode = m; idle = 0; wrongWay = 0; sought = 0
        autoButton?.title = m == .auto ? "停止滚动" : "自动滚动"
        if m != .manual { backButton?.isHidden = true }
    }
    /// 选区中心在整个桌面里的位置（左上角为原点）
    private var center: CGPoint {
        let primaryH = NSScreen.screens.first?.frame.height ?? screen.frame.height
        return CGPoint(x: screen.frame.minX + rect.midX, y: primaryH - (screen.frame.minY + rect.midY))
    }
    /// 滚轮事件是发给鼠标底下那个窗口的，所以鼠标得在选区里
    private func pointerInside() -> Bool {
        let p = NSEvent.mouseLocation
        return NSRect(x: screen.frame.minX + rect.minX, y: screen.frame.minY + rect.minY, width: rect.width, height: rect.height).contains(p)
    }
    private func scroll(_ points: CGFloat, forward: Bool) {
        let amount = Int32(points.rounded()) * (forward ? down : -down)
        guard let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: amount, wheel2: 0, wheel3: 0) else { return }
        e.location = center
        e.post(tap: .cghidEventTap)
    }
    /// 每截完一帧，按当前的模式决定要不要替用户滚一下
    private func drive(_ step: Stitcher.Step) {
        guard mode != .manual else { return }
        guard pointerInside() else {
            status.stringValue = "鼠标移出了选区，已停止。把鼠标放回框里再点一次"
            setMode(.manual); return
        }
        switch mode {
        case .auto:
            if case .lost = step { setMode(.manual); status.stringValue = "没对上，已停止自动滚动。点“回到断点”再继续"; backButton?.isHidden = false; return }
            if wrongWay >= 3 { down = -down; wrongWay = 0 }           // 这台机器的滚动方向是反的：换过来
            if idle >= 8 { setMode(.manual); status.stringValue = "滚到底了（或这里滚不动），点“完成”"; return }
            scroll(max(40, rect.height * 0.18), forward: true)
        case .seeking:
            switch step {
            case .lost:
                sought += 1
                if sought > 80 { setMode(.manual); status.stringValue = "没找到断点。请自己往回滚到已接上的内容附近"; backButton?.isHidden = false; return }
                scroll(max(40, rect.height * 0.22), forward: false)
            default:
                setMode(.manual); status.stringValue = "已回到断点，继续向下滚（慢一点），或点“自动滚动”"
            }
        case .manual: break
        }
    }
    @objc private func autoTapped() {
        if mode == .auto { setMode(.manual); status.stringValue = "已停止自动滚动"; return }
        guard canScroll() else { return }
        CGWarpMouseCursorPosition(center)
        setMode(.auto)
        status.stringValue = "正在自动向下滚动；想停就点“停止滚动”或把鼠标移出框"
    }
    @objc private func backTapped() {
        guard canScroll() else { return }
        CGWarpMouseCursorPosition(center)
        setMode(.seeking)
        status.stringValue = "正在往回找断点…"
    }

    @objc private func cancelTapped() { onEnd() }
    @objc private func saveTapped() { finish(save: true) }
    @objc private func doneTapped() { finish(save: false) }

    private func finish(save: Bool) {
        timer?.invalidate(); timer = nil
        if let cg = stitcher.result() {
            let scale = screen.backingScaleFactor
            deliverShot(cg, pointSize: NSSize(width: CGFloat(cg.width) / scale, height: CGFloat(cg.height) / scale), save: save, dir: saveDir)
        }
        onEnd()
    }

    func close() {
        closed = true
        timer?.invalidate(); timer = nil
        frameWin?.orderOut(nil); panel?.orderOut(nil)
        frameWin = nil; panel = nil
    }
}

// MARK: 自检：拼接算法（不需要屏幕）
// 造一张 5000 像素高的图，模拟一段一段往下滚，看拼出来的是不是和原图一模一样。
// 第二轮在每一帧上再盖一条固定的顶栏和底栏（像聊天窗口的标题栏和输入框），看底栏是不是只出现一次。

private func rgbaBytes(_ img: CGImage) -> [UInt8] {
    var b = [UInt8](repeating: 0, count: img.width * img.height * 4)
    b.withUnsafeMutableBytes { raw in
        let c = CGContext(data: raw.baseAddress, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: img.width * 4,
                          space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        c?.interpolationQuality = .none
        c?.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
    }
    return b
}

func runStitchSelfTest() -> Bool {
    let w = 800, total = 5000, frameH = 900
    guard let ctx = CGContext(data: nil, width: w, height: total, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
    ctx.setFillColor(CGColor(gray: 1, alpha: 1)); ctx.fill(CGRect(x: 0, y: 0, width: w, height: total))
    var seed: UInt64 = 20261112
    func rnd(_ n: Int) -> Int { seed = seed &* 6364136223846793005 &+ 1442695040888963407; return Int((seed >> 33) % UInt64(n)) }
    for _ in 0..<420 {                                   // 像网页那样：一行行深浅不一的“文字条”和色块
        ctx.setFillColor(CGColor(red: CGFloat(rnd(200)) / 255, green: CGFloat(rnd(200)) / 255, blue: CGFloat(rnd(200)) / 255, alpha: 1))
        ctx.fill(CGRect(x: rnd(w - 100), y: rnd(total), width: 40 + rnd(500), height: 6 + rnd(30)))
    }
    guard let tall = ctx.makeImage() else { return false }

    // 在一张图的顶部、底部盖上固定栏
    func withBars(_ img: CGImage, header: Int, footer: Int) -> CGImage? {
        guard header > 0 || footer > 0 else { return img }
        guard let c = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        c.interpolationQuality = .none
        c.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
        c.setFillColor(CGColor(red: 0.2, green: 0.25, blue: 0.3, alpha: 1)); c.fill(CGRect(x: 0, y: img.height - header, width: img.width, height: header))
        c.setFillColor(CGColor(red: 0.95, green: 0.8, blue: 0.3, alpha: 1)); c.fill(CGRect(x: 0, y: 0, width: img.width, height: footer))
        c.setFillColor(CGColor(gray: 0.1, alpha: 1)); c.fill(CGRect(x: 40, y: footer / 3, width: 300, height: max(1, footer / 3)))
        return c.makeImage()
    }

    // 左右各盖一条不动的侧栏（带分隔线），像聊天软件那样只有中间在滚
    let sideL = 200, sideR = 100
    func withSides(_ img: CGImage) -> CGImage? {
        guard let c = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        c.interpolationQuality = .none
        c.draw(img, in: CGRect(x: 0, y: 0, width: img.width, height: img.height))
        c.setFillColor(CGColor(gray: 0.96, alpha: 1))
        c.fill(CGRect(x: 0, y: 0, width: sideL, height: img.height)); c.fill(CGRect(x: img.width - sideR, y: 0, width: sideR, height: img.height))
        c.setFillColor(CGColor(gray: 0.35, alpha: 1))
        for i in 0..<12 { c.fill(CGRect(x: 16, y: 40 + i * 64, width: 150, height: 18)) }                 // 侧栏里的“菜单项”
        c.fill(CGRect(x: sideL - 1, y: 0, width: 1, height: img.height)); c.fill(CGRect(x: img.width - sideR, y: 0, width: 1, height: img.height))
        return c.makeImage()
    }

    func round(_ name: String, header: Int, footer: Int, sides: Bool = false) -> Bool {
        let st = Stitcher()
        var offset = 0, ok = true, frames = 0, step = 0
        while true {
            guard let crop = tall.cropping(to: CGRect(x: 0, y: offset, width: w, height: frameH)), let barred = withBars(crop, header: header, footer: footer),
                  let frame = sides ? withSides(barred) : barred else { return false }
            switch st.add(frame) {
            case .first: break
            case .added(let d): if d != step { ok = false; print("失败 \(name) 第 \(frames) 帧：实际滚了 \(step) 行，算出来 \(d) 行") }
            default: ok = false; print("失败 \(name) 第 \(frames) 帧没接上")
            }
            frames += 1
            if frames % 5 == 0 { if case .same = st.add(frame) {} else { ok = false; print("失败 \(name) 没滚动的一帧被当成滚动了") } }
            step = 60 + rnd(380)
            if offset + step + frameH > total { break }
            offset += step
        }
        let wantH = offset + frameH
        guard let out = st.result(), let plain = tall.cropping(to: CGRect(x: 0, y: 0, width: w, height: wantH)), let barred = withBars(plain, header: header, footer: footer),
              let want = sides ? barred.cropping(to: CGRect(x: sideL, y: 0, width: w - sideL - sideR, height: wantH)) : barred else { return false }
        if sides && out.width != w - sideL - sideR { ok = false; print("失败 \(name)：应只接中间 \(w - sideL - sideR) 像素宽的那一栏，实际 \(out.width)") }
        let same = out.height == wantH && out.width == want.width && rgbaBytes(out) == rgbaBytes(want)
        if !same { ok = false }
        let raw = out.width * wantH * 4
        print("\(ok ? "通过" : "失败") 长截屏拼接（\(name)）：\(frames) 帧，拼成 \(out.height) 像素高，应为 \(wantH)，和原图\(same ? "完全一致" : "不一致")；过程中占 \(st.storedBytes / 1024) KB（不压缩要 \(raw / 1024) KB）")
        return ok
    }
    let a = round("普通页面", header: 0, footer: 0)
    let b = round("有固定顶栏和底栏", header: 70, footer: 96)
    let c = round("两边有不动的侧栏", header: 0, footer: 0, sides: true)
    return a && b && c
}
