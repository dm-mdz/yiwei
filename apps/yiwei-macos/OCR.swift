// 一苇 · 截图里的“提取文字”：认出图里的字和每个字的位置（用系统自带的识别，在本机算，不联网）
import Cocoa
import Vision

/// 认出来的一个字。box：在图里的位置，按比例算（0–1），原点在左下角；同一段的字上下对齐（都用这一段的上下边）。
/// seg：第几段（系统认出来的一条文字）；row：第几排（差不多一样高的几段算一排，从上往下数）
struct OCRGlyph { let ch: Character; let box: CGRect; let seg: Int; let row: Int }

enum TextFinder {
    /// 认出图里的字，按从上到下、从左到右排好。要算一会儿（一般不到半秒），别在主线程上调
    static func find(in image: CGImage) -> [OCRGlyph] {
        let req = VNRecognizeTextRequest()
        req.recognitionLevel = .accurate
        req.recognitionLanguages = ["zh-Hans", "en-US"]
        guard (try? VNImageRequestHandler(cgImage: image, options: [:]).perform([req])) != nil else { return [] }
        var segs: [(text: VNRecognizedText, box: CGRect)] = []
        for o in req.results ?? [] { if let t = o.topCandidates(1).first, !t.string.isEmpty { segs.append((t, o.boundingBox)) } }
        // 排成一排一排：先按高低排，中线差不到半个字高的算同一排，排里再从左到右
        segs.sort { $0.box.midY > $1.box.midY }
        var rows: [[(text: VNRecognizedText, box: CGRect)]] = []
        for s in segs {
            if let last = rows.last?.first, abs(last.box.midY - s.box.midY) < min(last.box.height, s.box.height) * 0.5 { rows[rows.count - 1].append(s) }
            else { rows.append([s]) }
        }
        var out: [OCRGlyph] = []
        var seg = 0
        for (r, row) in rows.enumerated() {
            for s in row.sorted(by: { $0.box.minX < $1.box.minX }) {
                for (ch, x0, x1) in spread(s.text, in: s.box) {
                    out.append(OCRGlyph(ch: ch, box: CGRect(x: x0, y: s.box.minY, width: max(0, x1 - x0), height: s.box.height), seg: seg, row: r))
                }
                seg += 1
            }
        }
        return out
    }

    /// 一段文字里每个字的左右边。系统给中文是一个字一个框，给英文是一个词一个框：同一个框里的几个字按字宽分
    private static func spread(_ t: VNRecognizedText, in whole: CGRect) -> [(Character, CGFloat, CGFloat)] {
        let s = t.string
        var chars: [(ch: Character, box: CGRect?)] = []
        var i = s.startIndex
        while i < s.endIndex {
            let j = s.index(after: i)
            // 空格这类没有笔画的，系统给的是个空框：当成没给
            let b = (try? t.boundingBox(for: i..<j))?.boundingBox
            chars.append((s[i], (b?.width ?? 0) > 0 ? b : nil))
            i = j
        }
        let font = NSFont.systemFont(ofSize: 12)
        let weight: (Character) -> CGFloat = { max(1, (String($0) as NSString).size(withAttributes: [.font: font]).width) }
        var out: [(Character, CGFloat, CGFloat)] = []
        var k = 0
        while k < chars.count {
            // 连着的、框一样的（或者都没给框的）是一组
            var e = k + 1
            while e < chars.count, chars[e].box == chars[k].box { e += 1 }
            var x0: CGFloat, x1: CGFloat
            if let b = chars[k].box { x0 = b.minX; x1 = b.maxX }
            else {
                // 没给框的（多半是空格）：夹在前后两个框之间；前后也没有就按整段算
                x0 = out.last?.2 ?? whole.minX
                x1 = e < chars.count ? (chars[e].box?.minX ?? whole.maxX) : whole.maxX
            }
            if x1 < x0 { x1 = x0 }
            let ws = chars[k..<e].map { weight($0.ch) }, total = ws.reduce(0, +)
            var x = x0
            for (n, c) in chars[k..<e].enumerated() {
                let w = (x1 - x0) * ws[n] / total
                out.append((c.ch, x, x + w)); x += w
            }
            k = e
        }
        return out
    }

    /// 把选中的一串字拼成文字：换排换行，同一排里换段隔一个空格
    static func text(of glyphs: ArraySlice<OCRGlyph>) -> String {
        var s = ""
        var prev: OCRGlyph?
        for g in glyphs {
            if let p = prev { if g.row != p.row { s += "\n" } else if g.seg != p.seg { s += " " } }
            s.append(g.ch); prev = g
        }
        return s
    }

    /// 自检：画一张有中文、英文、数字的图，认出来对不对、每个字的位置是不是从左到右
    static func selfTest() -> Bool {
        let w = 900, h = 260
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        ctx.setFillColor(CGColor(gray: 1, alpha: 1)); ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 36), .foregroundColor: NSColor.black]
        ("键鼠共享本周内验收完" as NSString).draw(at: NSPoint(x: 40, y: 170), withAttributes: attrs)
        ("Release notes 2026 clipboard" as NSString).draw(at: NSPoint(x: 40, y: 100), withAttributes: attrs)
        ("电话 010-5555-0199" as NSString).draw(at: NSPoint(x: 40, y: 30), withAttributes: attrs)
        NSGraphicsContext.restoreGraphicsState()
        guard let img = ctx.makeImage() else { return false }
        let t0 = ProcessInfo.processInfo.systemUptime
        let g = find(in: img)
        let ms = Int((ProcessInfo.processInfo.systemUptime - t0) * 1000)
        let lines = text(of: g[...]).split(separator: "\n").map(String.init)
        var ok = true
        func check(_ what: String, _ cond: Bool) { print("\(cond ? "通过" : "没通过") 提取文字：\(what)"); if !cond { ok = false } }
        check("三行都认出来了，用了 \(ms) 毫秒：\(lines)", lines.count == 3 && lines[0] == "键鼠共享本周内验收完" && lines[1] == "Release notes 2026 clipboard" && lines[2].replacingOccurrences(of: " ", with: "") == "电话010-5555-0199")
        // 同一排里每个字的左边都不在前一个字的左边之前，而且都落在图里
        var ordered = true
        for i in 1..<max(1, g.count) where g[i].row == g[i - 1].row { if g[i].box.minX < g[i - 1].box.minX - 0.001 { ordered = false } }
        check("每个字都有位置，同一排从左到右", !g.isEmpty && ordered && g.allSatisfy { $0.box.minX >= -0.01 && $0.box.maxX <= 1.01 && $0.box.height > 0 })
        // 第一行第 3 个字“共”大约在 40 + 2 个字宽的地方
        if let c = g.first(where: { $0.ch == "共" }) {
            let x = c.box.midX * CGFloat(w)
            check("“共”字的位置对（图上约 \(Int(x))，应在 112–148 之间）", x > 112 && x < 148)
        } else { check("找到“共”字", false) }
        check("选中一部分拼出来的字对", text(of: g.filter { $0.row == 1 }.prefix(7)[...]) == "Release")
        return ok
    }
}
