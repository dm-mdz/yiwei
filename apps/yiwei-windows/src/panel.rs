//! 点托盘图标弹出的面板：和 Mac 菜单栏里的下拉界面一样（键鼠共享开关、位置小图、调整位置、发现的电脑、
//! 添加电脑、开机时自动启动、退出一苇）。全部自己画（GDI+），跟随系统的浅色、深色。
use crate::win::*;
use crate::{app, autostart, cancel_job, cs, join, local_ip, recv_dir, running, send_files, set_autostart, set_recv_dir, start_share, stop_share, tray, with_core, MAIN};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::Ordering::SeqCst;
use yiwei_core::ffi::*;

const W: f32 = 300.0;

#[derive(Clone, PartialEq, Debug)]
enum Act { Share, Expand, Join(String), Manual, NewCode, TakeBack, Auto, Quit, Cancel(String) }

#[derive(Default)]
struct Panel {
    hwnd: HWND, edit: HWND, edit_for: Option<String>, expanded: bool, dark: bool, scale: f32, height: f32,
    hits: Vec<(RectF, Act)>, hover: Option<Act>, pressed: Option<Act>,
    board: RectF, mini: RectF, selected: Option<String>, drag: Option<(String, f32, f32, f32, f32)>,   // 拖着哪张卡：编号、按下时离卡片左上角多远、现在鼠标在哪
    edit_rect: RectF, hidden_at: u32, anchor_bottom: Option<i32>, anchor_top: i32,
}
thread_local! { static P: RefCell<Panel> = RefCell::new(Panel { scale: 1.0, ..Default::default() }); }
fn p<R>(f: impl FnOnce(&mut Panel) -> R) -> R { P.with(|x| f(&mut x.borrow_mut())) }

// MARK: 颜色

struct C { bg: u32, text: u32, sub: u32, well: u32, card: u32, border: u32, hover: u32, accent: u32, knob_off: u32, sep: u32 }
const GOLD: u32 = 0xFFF2B705;
const RED: u32 = 0xFFC24A32;
fn colors(dark: bool) -> C {
    if dark { C { bg: 0xFF2B2B2B, text: 0xFFFFFFFF, sub: 0xFFA8A8A8, well: 0xFF333333, card: 0xFF404040, border: 0xFF5E5E5E, hover: 0xFF3A3A3A, accent: 0xFF4CC2FF, knob_off: 0xFFC8C8C8, sep: 0xFF404040 } }
    else { C { bg: 0xFFF9F9F9, text: 0xFF1A1A1A, sub: 0xFF62656A, well: 0xFFF0F0EF, card: 0xFFFFFFFF, border: 0xFFD4D4D1, hover: 0xFFEAEAEA, accent: 0xFF0067C0, knob_off: 0xFF5F5F5F, sep: 0xFFE3E3E0 } }
}
fn system_dark() -> bool {
    let mut v = 1u32; let mut n = 4u32;
    let k = w(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), w("AppsUseLightTheme").as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut v as *mut u32 as *mut c_void, &mut n); }
    v == 0
}

// MARK: 画图的小工具（坐标都是“逻辑像素”，画之前整体按缩放比例放大）

struct G { g: *mut c_void }
fn rect(x: f32, y: f32, w: f32, h: f32) -> RectF { RectF { x, y, w, h } }
fn inside(r: &RectF, x: f32, y: f32) -> bool { x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h }
thread_local! { static FAMILY: *mut c_void = unsafe {
    let mut f = std::ptr::null_mut();
    if GdipCreateFontFamilyFromName(w("Microsoft YaHei UI").as_ptr(), std::ptr::null_mut(), &mut f) != 0 { GdipGetGenericFontFamilySansSerif(&mut f); }
    f
}; }

impl G {
    fn rr(&self, r: RectF, rad: f32) -> *mut c_void {
        let mut path = std::ptr::null_mut();
        unsafe {
            GdipCreatePath(0, &mut path);
            let d = (rad * 2.0).min(r.w).min(r.h);
            if d <= 0.0 { GdipAddPathLine(path, r.x, r.y, r.x + r.w, r.y); GdipAddPathLine(path, r.x + r.w, r.y, r.x + r.w, r.y + r.h); GdipAddPathLine(path, r.x + r.w, r.y + r.h, r.x, r.y + r.h); }
            else {
                GdipAddPathArc(path, r.x, r.y, d, d, 180.0, 90.0);
                GdipAddPathArc(path, r.x + r.w - d, r.y, d, d, 270.0, 90.0);
                GdipAddPathArc(path, r.x + r.w - d, r.y + r.h - d, d, d, 0.0, 90.0);
                GdipAddPathArc(path, r.x, r.y + r.h - d, d, d, 90.0, 90.0);
            }
            GdipClosePathFigure(path);
        }
        path
    }
    fn fill_rr(&self, r: RectF, rad: f32, c: u32) {
        unsafe { let mut b = std::ptr::null_mut(); GdipCreateSolidFill(c, &mut b); let path = self.rr(r, rad); GdipFillPath(self.g, b, path); GdipDeletePath(path); GdipDeleteBrush(b); }
    }
    fn stroke_rr(&self, r: RectF, rad: f32, c: u32, width: f32, dash: bool) {
        unsafe {
            let mut pen = std::ptr::null_mut(); GdipCreatePen1(c, width, 0, &mut pen);
            if dash { GdipSetPenDashStyle(pen, 1); }
            let path = self.rr(r, rad); GdipDrawPath(self.g, pen, path); GdipDeletePath(path); GdipDeletePen(pen);
        }
    }
    fn ellipse(&self, r: RectF, c: u32) { unsafe { let mut b = std::ptr::null_mut(); GdipCreateSolidFill(c, &mut b); GdipFillEllipse(self.g, b, r.x, r.y, r.w, r.h); GdipDeleteBrush(b); } }
    fn fill(&self, r: RectF, c: u32) { unsafe { let mut b = std::ptr::null_mut(); GdipCreateSolidFill(c, &mut b); GdipFillRectangle(self.g, b, r.x, r.y, r.w, r.h); GdipDeleteBrush(b); } }
    /// align：0 靠左 1 居中 2 靠右；wrap：false 时一行，放不下用省略号
    fn text(&self, s: &str, r: RectF, size: f32, bold: bool, c: u32, align: i32, wrap: bool) {
        unsafe {
            let mut font = std::ptr::null_mut(); FAMILY.with(|f| GdipCreateFont(*f, size, bold as i32, 2, &mut font));
            let mut fmt = std::ptr::null_mut(); GdipCreateStringFormat(if wrap { 0 } else { 0x1000 }, 0, &mut fmt);
            GdipSetStringFormatAlign(fmt, align); GdipSetStringFormatLineAlign(fmt, if wrap { 0 } else { 1 }); GdipSetStringFormatTrimming(fmt, 3);
            let mut b = std::ptr::null_mut(); GdipCreateSolidFill(c, &mut b);
            let u = w(s); GdipDrawString(self.g, u.as_ptr(), -1, font, &r, fmt, b);
            GdipDeleteBrush(b); GdipDeleteStringFormat(fmt); GdipDeleteFont(font);
        }
    }
    fn measure(&self, s: &str, width: f32, size: f32) -> f32 {
        unsafe {
            let mut font = std::ptr::null_mut(); FAMILY.with(|f| GdipCreateFont(*f, size, 0, 2, &mut font));
            let mut fmt = std::ptr::null_mut(); GdipCreateStringFormat(0, 0, &mut fmt);
            let mut out = RectF::default(); let u = w(s);
            GdipMeasureString(self.g, u.as_ptr(), -1, font, &rect(0.0, 0.0, width, 10000.0), fmt, &mut out, std::ptr::null_mut(), std::ptr::null_mut());
            GdipDeleteStringFormat(fmt); GdipDeleteFont(font);
            out.h.ceil()
        }
    }
    fn switch(&self, x: f32, y: f32, on: bool, c: &C) {
        let r = rect(x, y, 38.0, 20.0);
        if on { self.fill_rr(r, 10.0, c.accent); self.ellipse(rect(x + 22.0, y + 4.0, 12.0, 12.0), 0xFFFFFFFF) }
        else { self.stroke_rr(rect(x + 0.5, y + 0.5, 37.0, 19.0), 9.5, c.knob_off, 1.0, false); self.ellipse(rect(x + 5.0, y + 5.0, 10.0, 10.0), c.knob_off) }
    }
}

// MARK: 三个系统的小图形（不用官方标志）：苹果、旗子、企鹅。路径在 24×24 的格子里

enum Seg { Move(f32, f32), Line(f32, f32), Cubic(f32, f32, f32, f32, f32, f32), Close }
const GLYPHS: [&[(&str, bool, f32)]; 3] = [
    &[("M12 8.2c-1.6-1.3-3.6-1.5-5.2-.5C4.9 8.9 4.2 11.2 4.7 13.8c.6 3 2.4 5.9 4.4 6.3 1.1.2 1.9-.5 2.9-.5s1.8.7 2.9.5c2-.4 3.8-3.3 4.4-6.3.5-2.6-.2-4.9-2.1-6.1-1.6-1-3.6-.8-5.2.5z", false, 1.6),
      ("M12 8.2c0-1.7.5-3 1.5-4", false, 1.6), ("M13.4 5.6c1.1-1.4 2.6-1.9 4-1.6-.2 1.6-1.5 2.8-3.4 2.7", false, 1.6)],
    &[("M3.5 7c2.8-1.6 5.4-1.1 8.2 0s5.4 1.5 8.8 0v10c-3.4 1.5-6 1.1-8.8 0s-5.4-1.6-8.2 0z", false, 1.6)],
    &[("M12 3.4c-3 0-4.5 2.4-4.5 5.3 0 1.7-.9 2.7-1.7 4.5-1 2.4 0 5.5 2 6.9h8.4c2-1.4 3-4.5 2-6.9-.8-1.8-1.7-2.8-1.7-4.5 0-2.9-1.5-5.3-4.5-5.3z", false, 1.6),
      ("M12 10.8c-1.9 0-2.9 1.9-2.9 4.3s1.1 4.4 2.9 4.4 2.9-2 2.9-4.4-1-4.3-2.9-4.3z", false, 1.6),
      ("M10.3 7.6h.01M13.7 7.6h.01", false, 2.0), ("M11.1 9.1h1.8L12 10.3z", true, 1.0), ("M8.6 20.6h2.4M13 20.6h2.4", false, 1.6)],
];
/// 只认这几个图形里用到的命令：M L H V C S Z（大小写都行）
fn parse_path(d: &str) -> Vec<Seg> {
    let ch: Vec<char> = d.chars().collect();
    let mut ops: Vec<(char, Vec<f32>)> = Vec::new();
    let (mut i, mut cmd, mut nums, mut started) = (0, 'M', Vec::new(), false);
    while i < ch.len() {
        let c = ch[i];
        if c.is_ascii_alphabetic() { if started { ops.push((cmd, std::mem::take(&mut nums))) } started = true; cmd = c; i += 1; continue }
        if c == ' ' || c == ',' { i += 1; continue }
        let (mut j, mut dot, mut t) = (i, false, String::new());
        if ch[j] == '-' || ch[j] == '+' { t.push(ch[j]); j += 1 }
        while j < ch.len() && (ch[j].is_ascii_digit() || (ch[j] == '.' && !dot)) { if ch[j] == '.' { dot = true; if t.is_empty() || t == "-" { t.push('0') } } t.push(ch[j]); j += 1 }
        if j == i { i += 1; continue }
        nums.push(t.parse().unwrap_or(0.0)); i = j;
    }
    if started { ops.push((cmd, nums)) }
    let mut out = Vec::new();
    let (mut cur, mut start, mut ctrl, mut curve) = ((0f32, 0f32), (0f32, 0f32), (0f32, 0f32), false);
    for (op, n) in ops {
        let rel = op.is_ascii_lowercase();
        let pt = |k: usize, cur: (f32, f32)| if rel { (cur.0 + n[k], cur.1 + n[k + 1]) } else { (n[k], n[k + 1]) };
        match op.to_ascii_uppercase() {
            'M' => { let mut k = 0; while k + 1 < n.len() { let q = pt(k, cur); out.push(if k == 0 { start = q; Seg::Move(q.0, q.1) } else { Seg::Line(q.0, q.1) }); cur = q; k += 2 } curve = false }
            'L' => { let mut k = 0; while k + 1 < n.len() { let q = pt(k, cur); out.push(Seg::Line(q.0, q.1)); cur = q; k += 2 } curve = false }
            'H' => { for v in &n { cur = (if rel { cur.0 + v } else { *v }, cur.1); out.push(Seg::Line(cur.0, cur.1)) } curve = false }
            'V' => { for v in &n { cur = (cur.0, if rel { cur.1 + v } else { *v }); out.push(Seg::Line(cur.0, cur.1)) } curve = false }
            'C' => { let mut k = 0; while k + 5 < n.len() { let (a, b, q) = (pt(k, cur), pt(k + 2, cur), pt(k + 4, cur)); out.push(Seg::Cubic(a.0, a.1, b.0, b.1, q.0, q.1)); ctrl = b; cur = q; k += 6 } curve = true }
            'S' => { let mut k = 0; while k + 3 < n.len() {
                let a = if curve { (2.0 * cur.0 - ctrl.0, 2.0 * cur.1 - ctrl.1) } else { cur };
                let (b, q) = (pt(k, cur), pt(k + 2, cur)); out.push(Seg::Cubic(a.0, a.1, b.0, b.1, q.0, q.1)); ctrl = b; cur = q; k += 4; curve = true } }
            'Z' => { out.push(Seg::Close); cur = start; curve = false }
            _ => {}
        }
    }
    out
}
fn glyph(g: &G, os: i32, r: RectF, color: u32) {
    let s = r.w / 24.0;
    let set = GLYPHS[match os { 2 => 1, 3 => 2, _ => 0 }];
    for (d, fill, width) in set {
        unsafe {
            let mut path = std::ptr::null_mut(); GdipCreatePath(0, &mut path);
            let t = |x: f32, y: f32| (r.x + x * s, r.y + y * s);
            let mut cur = (0f32, 0f32);
            for seg in parse_path(d) {
                match seg {
                    Seg::Move(x, y) => { GdipStartPathFigure(path); cur = t(x, y) }
                    Seg::Line(x, y) => {
                        let mut q = t(x, y);
                        if (q.0 - cur.0).abs() + (q.1 - cur.1).abs() < 0.05 { q.0 += 0.05 }    // 企鹅眼睛那种“点”：长度为 0 的线画不出来，拉长一丁点
                        GdipAddPathLine(path, cur.0, cur.1, q.0, q.1); cur = q
                    }
                    Seg::Cubic(a, b, c, d2, x, y) => { let (p1, p2, q) = (t(a, b), t(c, d2), t(x, y)); GdipAddPathBezier(path, cur.0, cur.1, p1.0, p1.1, p2.0, p2.1, q.0, q.1); cur = q }
                    Seg::Close => { GdipClosePathFigure(path); }
                }
            }
            if *fill { let mut b = std::ptr::null_mut(); GdipCreateSolidFill(color, &mut b); GdipFillPath(g.g, b, path); GdipDeleteBrush(b); }
            else {
                let mut pen = std::ptr::null_mut(); GdipCreatePen1(color, width * s, 0, &mut pen);
                GdipSetPenLineCap197819(pen, 2, 2, 2); GdipSetPenLineJoin(pen, 2);
                GdipDrawPath(g.g, pen, path); GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }
}

// MARK: 位置图

#[derive(Clone)]
struct Card { id: String, os: i32, label: String, x: i32, y: i32, me: bool, online: bool, lonely: bool }
fn cards(mini: bool) -> Vec<Card> {
    app(|a| {
        let me = a.store.id.clone();
        let mut list: Vec<Card> = a.store.cells.iter().map(|(id, x, y)| {
            let m = a.machines.get(id); let is_me = *id == me;
            Card { id: id.clone(), os: if is_me { 2 } else { m.map_or(0, |m| m.os) }, label: String::new(), x: *x, y: *y, me: is_me, online: is_me || m.map_or(false, |m| m.online), lonely: false }
        }).collect();
        if !list.iter().any(|c| c.me) { list.push(Card { id: me.clone(), os: 2, label: String::new(), x: 0, y: 0, me: true, online: true, lonely: false }) }
        let at = |l: &[Card], x: i32, y: i32| l.iter().any(|c| c.x == x && c.y == y);
        let snapshot = list.clone();
        for c in list.iter_mut() {
            c.lonely = snapshot.len() > 1 && ![(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|d| at(&snapshot, c.x + d.0, c.y + d.1));
            let os_name = ["电脑", "macOS", "Windows", "Linux"][c.os.clamp(0, 3) as usize];
            c.label = if mini { if c.me { "这台".into() } else { os_name.into() } }
                      else if c.me { "这台电脑".into() } else { a.machines.get(&c.id).map_or("另一台电脑".into(), |m| m.name.clone()) };
        }
        list
    })
}
struct Grid { cw: f32, ch: f32, x0: i32, y0: i32, ox: f32, oy: f32 }
fn grid(r: RectF, list: &[Card], mini: bool) -> Grid {
    let x0 = list.iter().map(|c| c.x).min().unwrap_or(0); let x1 = list.iter().map(|c| c.x).max().unwrap_or(0);
    let y0 = list.iter().map(|c| c.y).min().unwrap_or(0); let y1 = list.iter().map(|c| c.y).max().unwrap_or(0);
    let margin = if mini { 0.0 } else { 1.0 };                     // 大图四周各留半格，拖到那里就是往外加一格
    let cols = (x1 - x0 + 1) as f32 + margin; let rows = (y1 - y0 + 1) as f32 + margin;
    let (bw, bh) = if mini { (58.0, 64.0) } else { (74.0, 86.0) };
    let sc = 1f32.min(r.w / (cols * bw)).min(r.h / (rows * bh));
    let (cw, ch) = (bw * sc, bh * sc);
    Grid { cw, ch, x0, y0, ox: r.x + (r.w - cols * cw) / 2.0 + margin * cw / 2.0, oy: r.y + (r.h - rows * ch) / 2.0 + margin * ch / 2.0 }
}
fn draw_map(g: &G, r: RectF, mini: bool, c: &C, drag: &Option<(String, f32, f32, f32, f32)>, sel: &Option<String>) {
    let list = cards(mini); let gr = grid(r, &list, mini);
    let sq = if mini { 42.0 } else { 52.0 } * gr.cw / if mini { 58.0 } else { 74.0 };
    let origin = |k: &Card| -> (f32, f32) {
        if let Some((id, gx, gy, mx, my)) = drag { if *id == k.id { return (mx - gx, my - gy) } }
        (gr.ox + (k.x - gr.x0) as f32 * gr.cw, gr.oy + (k.y - gr.y0) as f32 * gr.ch)
    };
    let square = |k: &Card| { let o = origin(k); rect(o.0 + (gr.cw - sq) / 2.0, o.1 + 4.0, sq, sq) };
    let dragging = |k: &Card| drag.as_ref().map_or(false, |d| d.0 == k.id);
    {                                                               // 挨着、而且都连着的两台之间画黄线：鼠标能从这一侧穿过去；没连上的不画
        for a in list.iter().filter(|k| !dragging(k) && k.online) { for b in list.iter().filter(|k| !dragging(k) && k.online) {
            let (ra, rb) = (square(a), square(b));
            if b.x == a.x + 1 && b.y == a.y { g.fill_rr(rect(ra.x + ra.w + 2.0, ra.y + ra.h / 2.0 - 2.0, rb.x - ra.x - ra.w - 4.0, 4.0), 2.0, GOLD) }
            if b.y == a.y + 1 && b.x == a.x { g.fill_rr(rect(ra.x + 4.0, ra.y + ra.h + 3.0, 4.0, rb.y - ra.y - ra.h - 6.0), 2.0, GOLD) }
        } }
    }
    let mut order: Vec<&Card> = list.iter().filter(|k| !dragging(k)).collect();
    order.extend(list.iter().filter(|k| dragging(k)));
    for k in order {
        let rr = square(k);
        let fade = |col: u32| if k.online { col } else { (col & 0x00FF_FFFF) | 0x7300_0000 };
        if dragging(k) { g.fill_rr(rect(rr.x + 2.0, rr.y + 5.0, rr.w, rr.h), sq * 0.22, 0x40000000) }
        g.fill_rr(rr, sq * 0.22, fade(c.card));
        let lonely = k.lonely && !mini;
        if k.me { g.stroke_rr(rect(rr.x + 1.0, rr.y + 1.0, rr.w - 2.0, rr.h - 2.0), sq * 0.2, GOLD, if mini { 2.0 } else { 2.5 }, false) }
        else if lonely { g.stroke_rr(rect(rr.x + 0.75, rr.y + 0.75, rr.w - 1.5, rr.h - 1.5), sq * 0.21, RED, 1.5, true) }
        else { g.stroke_rr(rect(rr.x + 0.5, rr.y + 0.5, rr.w - 1.0, rr.h - 1.0), sq * 0.22, fade(c.border), 1.0, false) }
        if mini && sel.as_ref() == Some(&k.id) { g.stroke_rr(rect(rr.x - 3.0, rr.y - 3.0, rr.w + 6.0, rr.h + 6.0), sq * 0.26, c.accent, 2.0, false) }   // 左键选中的：一圈蓝色
        let gs = sq * if mini { 0.48 } else { 0.54 };
        glyph(g, k.os, rect(rr.x + (rr.w - gs) / 2.0, rr.y + (rr.h - gs) / 2.0, gs, gs), fade(if lonely { RED } else { c.text }));
        let size = if mini { 10.5 } else { 11.5 };
        let lw = (gr.cw - 2.0).max(sq + 16.0);
        g.text(&k.label, rect(rr.x + rr.w / 2.0 - lw / 2.0, rr.y + rr.h + 2.0, lw, size + 6.0), size, k.me, if k.me { c.text } else { c.sub }, 1, false);
        if !mini && !k.online { g.text("未连接", rect(rr.x + rr.w / 2.0 - gr.cw / 2.0, rr.y + rr.h + size + 6.0, gr.cw, 14.0), 10.5, false, c.sub, 1, false) }
    }
}
/// 大图下面那句话：只在有问题时说（谁没挨着、鼠标到不了），没问题就不说
fn sentence() -> (String, bool) {
    app(|a| {
        let me = &a.store.id; let cells = &a.store.cells;
        if cells.len() < 2 || !cells.iter().any(|c| &c.0 == me) { return ("其他电脑打开键鼠共享后会出现在这里".to_string(), false) }
        let at = |x: i32, y: i32| cells.iter().any(|c| c.1 == x && c.2 == y);
        let lonely: Vec<String> = cells.iter().filter(|c| ![(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|d| at(c.1 + d.0, c.2 + d.1)))
            .map(|c| if &c.0 == me { "这台".to_string() } else { format!("「{}」", a.machines.get(&c.0).map_or("另一台电脑".to_string(), |m| m.name.clone())) }).collect();
        if lonely.is_empty() { return (String::new(), false) }
        (format!("{}没挨着别的电脑，鼠标到不了", lonely.join("、")), true)
    })
}

// MARK: 整个面板：画一遍，顺便记下能点的地方，返回总高度

fn paint(g: &G, pn: &mut Panel) -> f32 {
    let c = colors(pn.dark);
    let on = running();
    let mut hits: Vec<(RectF, Act)> = Vec::new();
    let hover = pn.hover.clone();
    let row_bg = |r: RectF, act: &Act| if hover.as_ref() == Some(act) { g.fill_rr(r, 6.0, c.hover) };
    let mut y = 8.0;
    // 键鼠共享
    let (n, problem, found, ip) = app(|a| (a.machines.values().filter(|m| m.online).count(), a.problem.clone(),
                                          a.found.iter().map(|f| (f.id.clone(), f.name.clone(), f.joining)).collect::<Vec<_>>(), ()));
    let _ = ip;
    let sub = match (on, &problem) { (_, Some(p)) => p.clone(), (false, None) => "已关闭".into(),
                                     (true, None) if n > 0 => format!("已连接 {n} 台"), _ => "未连接".into() };
    let r = rect(8.0, y, W - 16.0, 50.0);
    row_bg(r, &Act::Share); hits.push((r, Act::Share));
    g.text("键鼠共享", rect(16.0, y + 7.0, 220.0, 20.0), 13.5, true, c.text, 0, false);
    g.text(&sub, rect(16.0, y + 27.0, 226.0, 18.0), 12.0, false, c.sub, 0, false);
    g.switch(W - 16.0 - 38.0, y + 15.0, on, &c);
    y += 52.0;
    if on {
        // 灰底的一块：位置图
        let inner = rect(18.0, 0.0, W - 36.0, 0.0);
        let (say, warn) = sentence();
        let say_h = if pn.expanded && !say.is_empty() { g.measure(&say, inner.w, 12.0) } else { 0.0 };
        let say_gap = if say_h > 0.0 { 6.0 } else { 0.0 };
        let (jobs, more): (Vec<crate::Job>, usize) = app(|a| (a.jobs.iter().rev().take(3).rev().cloned().collect(), a.jobs.len().saturating_sub(3)));
        let jobs_h = jobs.len() as f32 * 34.0 + if more > 0 { 18.0 } else { 0.0 };
        let well_h = jobs_h + if pn.expanded { 10.0 + 240.0 + 6.0 + 20.0 + 6.0 + say_h + say_gap + 26.0 + found.len() as f32 * 30.0 + 22.0 + 28.0 + 8.0 } else { 10.0 + 74.0 + 6.0 + 20.0 + 8.0 };
        g.fill_rr(rect(8.0, y, W - 16.0, well_h), 8.0, c.well);
        let mut iy = y + 10.0;
        let link = |label: &str, iy: f32, act: Act, hits: &mut Vec<(RectF, Act)>| {
            let r = rect(W - 18.0 - 90.0, iy, 90.0, 20.0);
            g.text(label, r, 12.0, true, if hover.as_ref() == Some(&act) { c.accent } else { c.text }, 2, false); hits.push((r, act));
        };
        if !pn.expanded {
            pn.mini = rect(inner.x, iy, inner.w, 74.0);
            draw_map(g, pn.mini, true, &c, &None, &pn.selected); iy += 80.0;
            if let Some(id) = &pn.selected {                               // 选中了一台：面板固定住，提示可以把文件拖进来
                let who = app(|a| a.machines.get(id).map_or("它".to_string(), |m| m.name.clone()));
                g.text(&format!("拖文件进来，发给{who}"), rect(inner.x, iy, inner.w - 60.0, 20.0), 12.0, false, c.sub, 0, false);
            }
            link("展开", iy, Act::Expand, &mut hits);
            if !found.is_empty() { g.ellipse(rect(W - 15.0, iy + 3.0, 6.0, 6.0), RED) }   // 发现了新电脑：“展开”旁亮小红点
        } else {
            pn.board = rect(inner.x, iy, inner.w, 240.0);
            draw_map(g, pn.board, false, &c, &pn.drag, &None); iy += 246.0;
            g.text("拖动卡片摆放", rect(inner.x, iy, 170.0, 20.0), 12.0, false, c.sub, 0, false);
            link("收起", iy, Act::Expand, &mut hits); iy += 26.0;
            if say_h > 0.0 { g.text(&say, rect(inner.x, iy, inner.w, say_h + 2.0), 12.0, false, if warn { RED } else { c.sub }, 0, true); iy += say_h + say_gap; }
            // 这一组的校验码：别的电脑要加入时输入它
            let code = app(|a| a.code.clone());
            let code = if code.len() == 8 { format!("{}-{}", &code[..4], &code[4..]) } else { code };
            g.text(&format!("校验码 {code}"), rect(inner.x, iy, inner.w - 90.0, 20.0), 12.0, false, c.sub, 0, false);
            link("换一个", iy, Act::NewCode, &mut hits); iy += 26.0;
            for (id, name, joining) in &found {
                g.text(name, rect(inner.x, iy, inner.w - 80.0, 24.0), 12.5, false, c.text, 0, false);
                let b = rect(W - 18.0 - 72.0, iy + 1.0, 72.0, 22.0);
                let act = Act::Join(id.clone());
                g.fill_rr(b, 5.0, if *joining { c.well } else if hover.as_ref() == Some(&act) { c.hover } else { c.card });
                g.stroke_rr(b, 5.0, c.border, 1.0, false);
                g.text(if *joining { "等对方同意…" } else { "加入" }, b, 12.0, false, if *joining { c.sub } else { c.text }, 1, false);
                if !joining { hits.push((b, act)) }
                iy += 30.0;
            }
            pn.edit_rect = rect(inner.x, iy, inner.w - 96.0, 20.0);
            if pn.edit == 0 { g.text(&format!("本机 {}", local_ip().unwrap_or_else(|| "没连网".into())), rect(inner.x, iy, inner.w - 90.0, 20.0), 11.5, false, c.sub, 0, false) }
            link(if pn.edit == 0 { "手动添加" } else { "加入" }, iy, Act::Manual, &mut hits);
            // 鼠标被控切回：开着时别的电脑的指针在这台上，这台自己的鼠标一动就请它回去
            iy += 28.0;
            let r = rect(inner.x, iy - 2.0, inner.w, 24.0);
            g.text("鼠标被控切回", rect(inner.x, iy, inner.w - 50.0, 20.0), 12.0, false, c.text, 0, false);
            g.switch(W - 18.0 - 38.0, iy, app(|a| a.store.take_back), &c);
            hits.push((r, Act::TakeBack));
        }
        // 正在发、收的文件：一行字，一根进度条，右边百分比和取消
        let mut jy = y + well_h - 8.0 - jobs_h;
        for j in &jobs {
            let pct = if j.total > 0 { j.done as f32 / j.total as f32 } else { 0.0 };
            let title = if j.state == 1 { if j.sending { format!("已{}", j.title) } else { j.title.replacen("收", "已收", 1) } } else { j.title.clone() };
            let right = match j.state { 2 => j.err.clone(), 1 => "完成".into(), _ => format!("{}%", (pct * 100.0) as i32) };
            g.text(&title, rect(inner.x, jy, inner.w - 86.0, 18.0), 12.0, false, c.text, 0, false);
            g.text(&right, rect(inner.x + inner.w - 84.0, jy, if j.state == 0 { 64.0 } else { 84.0 }, 18.0), 12.0, false, if j.state == 2 { RED } else { c.sub }, 2, false);
            if j.state == 0 {
                let x = rect(inner.x + inner.w - 16.0, jy, 16.0, 18.0); let act = Act::Cancel(j.key.clone());
                g.text("✕", x, 11.0, false, if hover.as_ref() == Some(&act) { c.text } else { c.sub }, 1, false); hits.push((x, act));
                g.fill_rr(rect(inner.x, jy + 22.0, inner.w, 4.0), 2.0, c.border);
                g.fill_rr(rect(inner.x, jy + 22.0, (inner.w * pct).max(4.0), 4.0), 2.0, GOLD);
            }
            jy += 34.0;
        }
        if more > 0 { g.text(&format!("还有 {more} 个"), rect(inner.x, jy, inner.w, 16.0), 11.5, false, c.sub, 0, false) }
        y += well_h + 4.0;
    }
    g.fill(rect(16.0, y + 4.0, W - 32.0, 1.0), c.sep); y += 9.0;
    let r = rect(8.0, y, W - 16.0, 36.0);
    row_bg(r, &Act::Auto); hits.push((r, Act::Auto));
    g.text("开机时自动启动", rect(16.0, y, 200.0, 36.0), 13.5, true, c.text, 0, false);
    g.switch(W - 16.0 - 38.0, y + 8.0, autostart(), &c);
    y += 38.0;
    let r = rect(8.0, y, W - 16.0, 30.0);
    row_bg(r, &Act::Quit); hits.push((r, Act::Quit));
    g.text("退出一苇", rect(16.0, y, 200.0, 30.0), 13.0, false, c.text, 0, false);
    y += 30.0 + 8.0;
    pn.hits = hits;
    y
}

/// 在一张内存画布上画（measure 为 true 时只为了算高度，不上屏）
fn render(pn: &mut Panel, dc: HANDLE, w_px: i32, h_px: i32) -> f32 {
    unsafe {
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w_px.max(1), h_px.max(1));
        let old = SelectObject(mem, bmp);
        let mut gp = std::ptr::null_mut(); GdipCreateFromHDC(mem, &mut gp);
        GdipSetSmoothingMode(gp, 4); GdipSetTextRenderingHint(gp, 5);
        GdipScaleWorldTransform(gp, pn.scale, pn.scale, 0);
        GdipGraphicsClear(gp, colors(pn.dark).bg);
        let g = G { g: gp };
        let h = paint(&g, pn);
        GdipDeleteGraphics(gp);
        BitBlt(dc, 0, 0, w_px, h_px, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old); DeleteObject(bmp); DeleteDC(mem);
        h
    }
}

// MARK: 窗口

fn ensure_window() -> HWND {
    let existing = p(|x| x.hwnd);
    if existing != 0 { return existing }
    unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { version: 1, callback: 0, no_thread: 0, no_codecs: 0 };
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("YiweiPanel");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: CS_DROPSHADOW, lpfnWndProc: Some(proc_), cbClsExtra: 0, cbWndExtra: 0,
                               hInstance: inst, hIcon: 0, hCursor: LoadCursorW(0, 32512), hbrBackground: 0, lpszMenuName: std::ptr::null(), lpszClassName: class.as_ptr(), hIconSm: 0 };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(), w("一苇").as_ptr(), WS_POPUP, 0, 0, 300, 200, 0, 0, inst, std::ptr::null());
        let round = 2u32; DwmSetWindowAttribute(h, 33, &round as *const u32 as *const c_void, 4);   // Windows 11 上用圆角
        DragAcceptFiles(h, 1);                                                                   // 能接住从资源管理器拖进来的文件
        p(|x| x.hwnd = h);
        h
    }
}

/// 重新算高度、摆位置。anchor：第一次弹出时按鼠标和任务栏的位置定
fn relayout(anchor: Option<POINT>) {
    let h = ensure_window();
    unsafe {
        if let Some(pt) = anchor {
            SetWindowPos(h, 0, pt.x, pt.y, 10, 10, SWP_NOZORDER | SWP_NOACTIVATE);     // 先挪到那块显示器上，才拿得到它的缩放比例
        }
        let scale = GetDpiForWindow(h).max(96) as f32 / 96.0;
        let dc = GetDC(h);
        let height = p(|pn| { pn.scale = scale; render(pn, dc, 1, 1) });
        ReleaseDC(h, dc);
        let (wp, hp) = ((W * scale).round() as i32, (height * scale).round() as i32);
        let mut x; let y;
        let pt = anchor.unwrap_or_else(|| { let mut q = POINT::default(); GetCursorPos(&mut q); q });
        let mut mi: MONITORINFO = std::mem::zeroed(); mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let (work, mon) = (mi.rcWork, mi.rcMonitor);
        if let Some(pt) = anchor {
            x = pt.x - wp / 2;
            // 任务栏在下面就贴着它往上弹，在上面就往下弹
            if work.top > mon.top { p(|pn| { pn.anchor_bottom = None; pn.anchor_top = work.top + 8 }) }
            else { p(|pn| pn.anchor_bottom = Some(work.bottom - 8)) }
            p(|pn| if pn.anchor_bottom.is_none() && work.bottom < mon.bottom { pn.anchor_bottom = Some(work.bottom - 8) });
        } else {
            let mut r = RECT::default(); crate::win_rect(h, &mut r); x = r.left;
        }
        x = x.clamp(work.left + 8, (work.right - wp - 8).max(work.left + 8));
        y = match p(|pn| pn.anchor_bottom) { Some(b) => (b - hp).max(work.top + 8), None => p(|pn| pn.anchor_top) };
        SetWindowPos(h, 0, x, y, wp, hp, SWP_NOZORDER | SWP_NOACTIVATE);
        p(|pn| { pn.height = height; if pn.edit != 0 { let r = pn.edit_rect; SetWindowPos(pn.edit, 0, (r.x * scale) as i32, (r.y * scale) as i32, (r.w * scale) as i32, (r.h * scale + 4.0) as i32, SWP_NOZORDER); } });
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

/// 点托盘图标：开着就关，关着就打开
pub fn toggle() {
    let h = ensure_window();
    unsafe {
        if IsWindowVisible(h) != 0 { hide(); return }
        if GetTickCount().wrapping_sub(p(|pn| pn.hidden_at)) < 300 { return }    // 刚因为点了托盘图标而失去焦点关掉的：别马上又打开
        p(|pn| { pn.dark = system_dark(); pn.hover = None; pn.drag = None });
        let mut pt = POINT::default(); GetCursorPos(&mut pt);
        relayout(Some(pt));
        ShowWindow(h, SW_SHOW);
        SetForegroundWindow(h);
    }
}
pub fn hide() {
    let (h, e) = p(|pn| { pn.selected = None; (pn.hwnd, std::mem::take(&mut pn.edit)) });
    unsafe { if e != 0 { DestroyWindow(e); } if h != 0 && IsWindowVisible(h) != 0 { ShowWindow(h, SW_HIDE); p(|pn| pn.hidden_at = GetTickCount()) } }
}
/// 状态变了（连上、断开、发现了新电脑……）：开着就重画
pub fn refresh() {
    let h = p(|pn| pn.hwnd);
    if h != 0 && unsafe { IsWindowVisible(h) } != 0 && p(|pn| pn.drag.is_none()) { relayout(None) }
}

fn act(a: Act) {
    match a {
        Act::Cancel(k) => cancel_job(&k),
        Act::Share => { if running() { stop_share() } else { start_share() } let en = running(); app(|a| { a.store.enabled = en; a.store.save() }); tray(NIM_MODIFY, None) }
        Act::Expand => p(|pn| pn.expanded = !pn.expanded),
        Act::Join(id) => edit_code(Some(id)),
        Act::Manual => manual(),
        Act::NewCode => {
            let text = "连着的组员会自动跟着换。\n现在没开机的电脑，之后要用新的校验码重新加入。";
            if unsafe { MessageBoxW(p(|pn| pn.hwnd), w(text).as_ptr(), w("换一个校验码？").as_ptr(), MB_YESNO | MB_ICONQUESTION) } == IDYES { with_core(|h| ywk_new_code(h)) }
        }
        Act::TakeBack => { let on = app(|a| { a.store.take_back = !a.store.take_back; a.store.save(); a.store.take_back }); with_core(|h| ywk_set_take_back(h, on as i32)) }
        Act::Auto => set_autostart(!autostart()),
        Act::Quit => { hide(); stop_share(); tray(NIM_DELETE, None); unsafe { PostQuitMessage(0) }; return }
    }
    relayout(None);
}

/// 手动添加：最下面那一行变成输入框，填“地址 校验码”，回车或点“加入”
fn manual() {
    if p(|pn| pn.edit) != 0 { submit_manual(); return }
    edit_code(None);
}
/// 最下面那一行变成输入框。who：给发现的哪台电脑填校验码；None 是手动添加（地址和校验码都填）
fn edit_code(who: Option<String>) {
    let (h, e, scale) = p(|pn| (pn.hwnd, pn.edit, pn.scale));
    if e != 0 { unsafe { DestroyWindow(e); } }
    let cue = match &who {
        Some(id) => format!("“{}”的校验码", app(|a| a.found.iter().find(|f| &f.id == id).map_or(String::new(), |f| f.name.clone()))),
        None => "地址 校验码，例如 192.168.1.20 K7M4-P2XQ".to_string(),
    };
    unsafe {
        let edit = CreateWindowExW(0, w("EDIT").as_ptr(), w("").as_ptr(), WS_CHILD | WS_VISIBLE | WS_BORDER | ES_AUTOHSCROLL, 0, 0, 10, 10, h, 0, GetModuleHandleW(std::ptr::null()), std::ptr::null());
        let font = CreateFontW(-(12.0 * scale) as i32, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w("Microsoft YaHei UI").as_ptr());
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        SendMessageW(edit, 0x1501, 1, w(&cue).as_ptr() as isize);      // EM_SETCUEBANNER：灰色提示字
        p(|pn| { pn.edit = edit; pn.edit_for = who });
        SetFocus(edit);
    }
}
/// 输入框里按了回车（主消息循环里转过来）
pub fn on_edit_key(hwnd: HWND, vk: usize) -> bool {
    let e = p(|pn| pn.edit);
    if e == 0 || hwnd != e { return false }
    match vk { 0x0D => { submit_manual(); true } 0x1B => { unsafe { DestroyWindow(e); } p(|pn| { pn.edit = 0; pn.edit_for = None }); relayout(None); true } _ => false }
}
fn submit_manual() {
    let e = p(|pn| pn.edit);
    let mut buf = [0u16; 64];
    let n = unsafe { GetWindowTextW(e, buf.as_mut_ptr(), 64) };
    let text = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    if let Some(id) = p(|pn| pn.edit_for.clone()) {
        // 给发现的一台电脑填的校验码
        if text.trim().is_empty() { return }
        join(&id, text.trim());
    } else {
        // 手动添加：“地址 校验码”
        let mut parts = text.split_whitespace();
        let (addr, code) = (parts.next().unwrap_or(""), parts.collect::<Vec<_>>().join(""));
        let (ip, port) = match addr.split_once(':') { Some((a, b)) => (a.to_string(), b.parse().unwrap_or(47800)), None => (addr.to_string(), 47800u16) };
        if ip.parse::<std::net::Ipv4Addr>().is_err() || code.is_empty() {
            unsafe { MessageBoxW(p(|pn| pn.hwnd), w("先填地址，空一格，再填那台电脑的校验码。\n例如 192.168.1.20 K7M4-P2XQ").as_ptr(), w("没填对").as_ptr(), MB_OK | MB_ICONWARNING); }
            return;
        }
        let (c, code) = (cs(&ip), cs(&code));
        with_core(|h| ywk_join(h, c.as_ptr(), port, code.as_ptr()));
    }
    unsafe { DestroyWindow(e); }
    p(|pn| { pn.edit = 0; pn.edit_for = None });
    relayout(None);
}

fn card_at(pn: &Panel, x: f32, y: f32) -> Option<(Card, f32, f32)> {
    let (area, mini) = if pn.expanded { (pn.board, false) } else { (pn.mini, true) };
    if !inside(&area, x, y) { return None }
    let list = cards(mini); let gr = grid(area, &list, mini);
    list.into_iter().find_map(|k| {
        let o = (gr.ox + (k.x - gr.x0) as f32 * gr.cw, gr.oy + (k.y - gr.y0) as f32 * gr.ch);
        inside(&rect(o.0, o.1, gr.cw, gr.ch), x, y).then(|| (k, x - o.0, y - o.1))
    })
}

unsafe extern "system" fn proc_(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pos = || { let s = p(|pn| pn.scale); ((lp & 0xFFFF) as i16 as f32 / s, ((lp >> 16) & 0xFFFF) as i16 as f32 / s) };
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let (wpx, hpx) = p(|pn| ((W * pn.scale).round() as i32, (pn.height * pn.scale).round() as i32));
            p(|pn| render(pn, dc, wpx, hpx));
            EndPaint(h, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        // 点到别处就收起；但选中了一台电脑时固定住，好从资源管理器把文件拖进来
        WM_ACTIVATE => { if wp & 0xFFFF == 0 && p(|pn| pn.selected.is_none()) { hide() } 0 }
        WM_DROPFILES => {
            let hdrop = wp as HANDLE;
            let mut files = Vec::new();
            let n = DragQueryFileW(hdrop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(hdrop, i, buf.as_mut_ptr(), len + 1);
                files.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            DragFinish(hdrop);
            crate::slog(&format!("文件拖进面板：{} 个", files.len()));
            // 拖到哪张卡片上就发给哪台；没对准卡片就发给选中的那台
            let mut q = POINT::default(); GetCursorPos(&mut q); let mut r = RECT::default(); crate::win_rect(h, &mut r);
            let s = p(|pn| pn.scale);
            let (x, y) = ((q.x - r.left) as f32 / s, (q.y - r.top) as f32 / s);
            let target = p(|pn| card_at(pn, x, y).filter(|(k, _, _)| !k.me && k.online).map(|(k, _, _)| k.id).or(pn.selected.clone()));
            if let Some(id) = target { send_files(&id, &files) }
            relayout(None);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = pos();
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            let changed = p(|pn| {
                if let Some(d) = pn.drag.as_mut() { d.3 = x; d.4 = y; return true }
                let hv = pn.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, a)| a.clone());
                let c = hv != pn.hover; pn.hover = hv; c
            });
            if changed { InvalidateRect(h, std::ptr::null(), 0); }
            0
        }
        WM_MOUSELEAVE => { if p(|pn| pn.hover.take().is_some()) { InvalidateRect(h, std::ptr::null(), 0); } 0 }
        WM_SETCURSOR => {
            let hand = p(|pn| pn.hover.is_some() || pn.drag.is_some()) || { let mut q = POINT::default(); GetCursorPos(&mut q); let mut r = RECT::default(); crate::win_rect(h, &mut r); let s = p(|pn| pn.scale); p(|pn| card_at(pn, (q.x - r.left) as f32 / s, (q.y - r.top) as f32 / s).is_some()) };
            SetCursor(LoadCursorW(0, if hand { 32649 } else { 32512 }));
            1
        }
        WM_LBUTTONDOWN => {
            let (x, y) = pos();
            // 小图上左键：选中一台连着的电脑（再点一下取消），之后按 Ctrl+V 把复制的文件发给它
            if p(|pn| !pn.expanded) {
                if let Some((k, _, _)) = p(|pn| card_at(pn, x, y)) {
                    // 选中状态下再点一次选中的那台：取消选中（面板不收起，恢复成点别处就收起）
                    if p(|pn| pn.selected.as_ref() == Some(&k.id)) { p(|pn| pn.selected = None); relayout(None); return 0 }
                    if !k.me && k.online { crate::slog(&format!("选中 {}，面板固定", k.label)); p(|pn| pn.selected = Some(k.id.clone())); relayout(None); }
                    return 0;
                }
            }
            let started = p(|pn| match card_at(pn, x, y).filter(|_| pn.expanded) { Some((k, gx, gy)) => { pn.drag = Some((k.id, gx, gy, x, y)); true } None => { pn.pressed = pn.hits.iter().find(|(r, _)| inside(r, x, y)).map(|(_, a)| a.clone()); false } });
            if started { SetCapture(h); InvalidateRect(h, std::ptr::null(), 0); }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = pos();
            if let Some((id, gx, gy, mx, my)) = p(|pn| pn.drag.take()) {
                ReleaseCapture();
                let list = cards(false); let board = p(|pn| pn.board); let gr = grid(board, &list, false);
                let cx = gr.x0 + ((mx - gx - gr.ox) / gr.cw).round() as i32; let cy = gr.y0 + ((my - gy - gr.oy) / gr.ch).round() as i32;
                if list.iter().find(|k| k.id == id).map_or(false, |k| (k.x, k.y) != (cx, cy)) { let c = cs(&id); with_core(|hh| ywk_place(hh, c.as_ptr(), cx, cy)) }
                relayout(None);
                return 0;
            }
            let a = p(|pn| { let a = pn.pressed.take(); a.filter(|a| pn.hits.iter().any(|(r, b)| b == a && inside(r, x, y))) });
            if let Some(a) = a { act(a) }
            0
        }
        WM_RBUTTONUP => {
            // 小图上右键：别的电脑“发送文件…”“发送文件夹…”，自己“收到的文件保存到…”
            let (x, y) = pos();
            let expanded = p(|pn| pn.expanded);
            if let Some((k, _, _)) = p(|pn| card_at(pn, x, y)).filter(|_| !expanded) {
                let menu = CreatePopupMenu();
                if k.me {
                    let d = recv_dir(); let name = std::path::Path::new(&d).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(d.clone());
                    let _ = name;
                    AppendMenuW(menu, MF_STRING, 3, w("保存位置").as_ptr());
                } else if k.online {
                    AppendMenuW(menu, MF_STRING, 1, w("发送文件").as_ptr());
                    AppendMenuW(menu, MF_STRING, 2, w("发送文件夹").as_ptr());
                }
                let mut q = POINT::default(); GetCursorPos(&mut q);
                let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, q.x, q.y, 0, h, std::ptr::null());
                DestroyMenu(menu);
                let who = app(|a| a.machines.get(&k.id).map_or(k.label.clone(), |m| m.name.clone()));
                match cmd { 1 => { let f = pick(h, false, true, &format!("发给“{who}”"), "发送", None); send_files(&k.id, &f) }
                            2 => { let f = pick(h, true, true, &format!("发给“{who}”"), "发送", None); send_files(&k.id, &f) }
                            3 => { let d = recv_dir(); let _ = std::fs::create_dir_all(&d); if let Some(p) = pick(h, true, false, "收到的文件放在哪里", "选这里", Some(&d)).pop() { set_recv_dir(p) } }
                            _ => {} }
                relayout(None);
                return 0;
            }
            // 大图上右键别的电脑的卡片：把它请出这一组（这一组随即换一个校验码，它拿着旧的进不来）
            if let Some((k, _, _)) = p(|pn| card_at(pn, x, y)) {
                if !k.me {
                    let menu = CreatePopupMenu();
                    AppendMenuW(menu, MF_STRING, 1, w(&format!("把“{}”请出这一组", k.label)).as_ptr());
                    let mut q = POINT::default(); GetCursorPos(&mut q);
                    let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, q.x, q.y, 0, h, std::ptr::null());
                    DestroyMenu(menu);
                    let sure = "这一组会换一个新的校验码，连着的组员自动跟着换；它拿着旧的进不来。\n现在没开机的其他电脑，之后也要用新校验码重新加入。";
                    if cmd == 1 && MessageBoxW(h, w(sure).as_ptr(), w(&format!("把“{}”请出这一组？", k.label)).as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES {
                        let c = cs(&k.id);
                        with_core(|hh| ywk_forget(hh, c.as_ptr()));
                        app(|a| { a.store.peers.retain(|p| p.id != k.id); a.machines.remove(&k.id); a.store.save() });
                        relayout(None);
                    }
                }
            }
            0
        }
        WM_DPICHANGED => { relayout(None); 0 }
        WM_KEYDOWN if wp == 0x56 && GetKeyState(0x11) < 0 => {
            // Ctrl+V：把资源管理器里复制的文件发给选中的那台
            if let Some(id) = p(|pn| pn.selected.clone()) {
                let files = clipboard_files(h);
                crate::slog(&format!("Ctrl+V：剪贴板里 {} 个文件", files.len()));
                if files.is_empty() { MessageBoxW(h, w("先在资源管理器里复制要发的文件").as_ptr(), w("剪贴板里没有文件").as_ptr(), MB_OK); } else { send_files(&id, &files) }
                relayout(None);
            }
            0
        }
        WM_COMMAND => { if (wp >> 16) as u32 == 0x0300 { /* EN_CHANGE */ } 0 }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

#[allow(dead_code)]
fn main_hwnd() -> HWND { MAIN.load(SeqCst) }

/// 新式的选择窗口（IFileOpenDialog）：folders 选文件夹，否则选文件；multi 可以多选。ok 是确认按钮上的字
fn pick(owner: HWND, folders: bool, multi: bool, title: &str, ok: &str, start: Option<&str>) -> Vec<String> {
    type Opt = unsafe extern "system" fn(*mut c_void, u32) -> i32;
    type GetOpt = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
    type Str = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
    type Ptr = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
    type Show = unsafe extern "system" fn(*mut c_void, HWND) -> i32;
    type Out = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
    type Count = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
    type At = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type Name = unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32;
    type Rel = unsafe extern "system" fn(*mut c_void) -> u32;
    unsafe fn f<T: Copy>(o: *mut c_void, i: usize) -> T { let vt = *(o as *const *const usize); std::mem::transmute_copy(&*vt.add(i)) }
    let mut out = Vec::new();
    unsafe {
        CoInitializeEx(std::ptr::null(), 2);
        let mut d: *mut c_void = std::ptr::null_mut();
        if CoCreateInstance(&CLSID_FILE_OPEN_DIALOG, std::ptr::null_mut(), 1, &IID_IFILE_OPEN_DIALOG, &mut d) != 0 || d.is_null() { return out }
        let mut o = 0u32; f::<GetOpt>(d, 10)(d, &mut o);
        f::<Opt>(d, 9)(d, o | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | if folders { FOS_PICKFOLDERS } else { 0 } | if multi { FOS_ALLOWMULTISELECT } else { 0 });
        f::<Str>(d, 17)(d, w(title).as_ptr());
        f::<Str>(d, 18)(d, w(ok).as_ptr());
        if let Some(st) = start {
            let mut item: *mut c_void = std::ptr::null_mut();
            if SHCreateItemFromParsingName(w(st).as_ptr(), std::ptr::null_mut(), &IID_ISHELL_ITEM, &mut item) == 0 && !item.is_null() { f::<Ptr>(d, 12)(d, item); f::<Rel>(item, 2)(item); }
        }
        if f::<Show>(d, 3)(d, owner) == 0 {
            let mut arr: *mut c_void = std::ptr::null_mut();
            if f::<Out>(d, 27)(d, &mut arr) == 0 && !arr.is_null() {
                let mut n = 0u32; f::<Count>(arr, 7)(arr, &mut n);
                for i in 0..n {
                    let mut item: *mut c_void = std::ptr::null_mut();
                    if f::<At>(arr, 8)(arr, i, &mut item) != 0 || item.is_null() { continue }
                    let mut pw: *mut u16 = std::ptr::null_mut();
                    if f::<Name>(item, 5)(item, SIGDN_FILESYSPATH, &mut pw) == 0 && !pw.is_null() {
                        let mut len = 0; while *pw.add(len) != 0 { len += 1 }
                        out.push(String::from_utf16_lossy(std::slice::from_raw_parts(pw, len)));
                        CoTaskMemFree(pw as *mut c_void);
                    }
                    f::<Rel>(item, 2)(item);
                }
                f::<Rel>(arr, 2)(arr);
            }
        }
        f::<Rel>(d, 2)(d);
    }
    out
}
/// 剪贴板里复制的文件（在资源管理器里 Ctrl+C 的那些）
pub(crate) fn clipboard_files(owner: HWND) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        // 刚复制完别的程序可能正占着剪贴板：等一等再试
        if !(0..10).any(|i| { if i > 0 { std::thread::sleep(std::time::Duration::from_millis(15)) } OpenClipboard(owner) != 0 }) { return out }
        let h = GetClipboardData(CF_HDROP);
        if h != 0 {
            let n = DragQueryFileW(h, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
            for i in 0..n {
                let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(h, i, buf.as_mut_ptr(), len + 1);
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
        }
        CloseClipboard();
    }
    out
}
