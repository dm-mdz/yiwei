//! 一苇 · Windows 版：只有键鼠共享。常驻在任务栏右下角（托盘），点图标出菜单。
//! 连网、入组、谁在控制谁都在核心（core/）里；这里管和 Windows 打交道的部分：
//!   · 监听本机键鼠（低级键鼠钩子），报给核心；鼠标穿到别的电脑时把本机鼠标停在主屏中间、换成透明指针，输入不再给本机。
//!     钩子里只调核心那几个“放进队列就返回”的函数：穿不穿屏由核心的后台线程判断，结果通过通知回来
//!   · 把别的电脑发来的输入注入本机（SendInput）
//!   · 剪贴板跟着指针走（读写本机的剪贴板；规则在核心的 clipflow 里）、托盘菜单（加入发现的电脑、开机自启、退出）、“允许加入吗”的确认框
//! 摆位置在 Mac 上拖（摆好会自动同步过来）；这边第一次连上时会自动摆在旁边。
#![windows_subsystem = "windows"]

mod json;
mod panel;
mod win;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering::SeqCst};
use std::sync::{Mutex, RwLock};
use win::*;
use yiwei_core::clipflow::{Board, Flow};
use yiwei_core::ffi::*;
use yiwei_core::keys::{usage_from_win, win_from_usage};
use yiwei_core::wire::ClipTag;

/// 自己注入的输入带这个记号，钩子里认出来就放过
const MARK: usize = 0x5957_4B31;
const WM_NOTICE: u32 = WM_APP + 1;     // lParam：Box<String>，核心发来的一行 JSON
const WM_TRAY: u32 = WM_APP + 2;
const WM_GRABBED: u32 = WM_APP + 3;    // 鼠标穿到别的电脑上了
const WM_RELEASED: u32 = WM_APP + 4;   // 鼠标回来了
const WM_PEER_LEFT: u32 = WM_APP + 5;  // 别的电脑的指针走了。wParam：0 正常走了、1 被请回去了、2 掉线了
const TIMER_PLACE: usize = 7;
const TIMER_WATCH: usize = 8;
const TIMER_SWEEP: usize = 9;
const TIMER_JOBS: usize = 10;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;

static MAIN: AtomicIsize = AtomicIsize::new(0);
static CORE: RwLock<usize> = RwLock::new(0);        // *mut Handle；大家都只拿读锁（核心自己是线程安全的），只有开关时拿写锁，钩子不会被别人堵住
static REMOTE: AtomicBool = AtomicBool::new(false);  // 外壳这边：本机指针是不是停在中间藏着。钩子里问的是核心（ywk_remote）
static PARK: (AtomicI32, AtomicI32) = (AtomicI32::new(0), AtomicI32::new(0));
static ORIGIN: (AtomicI32, AtomicI32) = (AtomicI32::new(0), AtomicI32::new(0));
static SIZE: (AtomicI32, AtomicI32) = (AtomicI32::new(1920), AtomicI32::new(1080));
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// 对方的鼠标走了以后，本机指针藏着、等本机鼠标动
static IDLE_HIDDEN: AtomicBool = AtomicBool::new(false);
/// 别的电脑的鼠标正在本机上（核心的线程上改）
static BOSSED: AtomicBool = AtomicBool::new(false);
const WM_SHOW_LOCAL: u32 = WM_APP + 6;
const WM_PEER_CAME: u32 = WM_APP + 8;  // 别的电脑的指针进来了
const WM_SET_CAPS: u32 = WM_APP + 7;   // wParam：对方的大写锁定开没开，本机跟它一致

// MARK: 存档（%APPDATA%\Yiwei\share.txt，一行一项）

#[derive(Default, Clone)]
struct Peer { id: String, key: String, os: i32, name: String }
#[derive(Default)]
struct Store { id: String, private: String, public: String, group: String, enabled: bool, rev: u64, cells: Vec<(String, i32, i32)>, peers: Vec<Peer>, recv_dir: String, take_back: bool }

/// 收到的文件放这里：存档里没写就用“下载\一苇”
fn recv_dir() -> String {
    let d = app(|a| a.store.recv_dir.clone());
    if !d.is_empty() { return d }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    format!("{home}\\Downloads\\一苇")
}
/// 记一行运行记录（Yiwei.exe 旁边的“一苇日志-电脑名.txt”，放不了就放 %LOCALAPPDATA%\Yiwei）。
/// 带上电脑名：几台电脑从同一个共享文件夹运行时各记各的
pub(crate) fn slog(t: &str) { let c = cs(t); ywk_log(c.as_ptr()) }
fn log_name() -> String {
    let pc: String = std::env::var("COMPUTERNAME").unwrap_or_default().chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    if pc.is_empty() { "一苇日志.txt".into() } else { format!("一苇日志-{pc}.txt") }
}
fn log_path() -> String {
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let p = d.join(log_name());
        if std::fs::OpenOptions::new().create(true).append(true).open(&p).is_ok() { return p.to_string_lossy().into_owned() }
    }
    data_dir().join(log_name()).to_string_lossy().into_owned()
}
fn data_dir() -> std::path::PathBuf {
    // 放本机目录（LOCALAPPDATA）：漫游配置文件会把编号和私钥带到别的电脑上
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("Yiwei")
}
impl Store {
    fn load() -> Store {
        let file = data_dir().join("share.txt");
        let mut s = Store { enabled: true, ..Default::default() };
        match std::fs::read_to_string(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                // 读不了（被占用之类）：这次先不连网，别生成新身份把存档覆盖掉
                s.enabled = false; s.id.clear(); return s;
            }
            _ => {}
        }
        if let Ok(text) = std::fs::read_to_string(&file) {
            for line in text.lines() {
                let p: Vec<&str> = line.splitn(5, ' ').collect();
                match p.as_slice() {
                    ["id", v] => s.id = v.to_string(),
                    ["key", a, b] => { s.private = a.to_string(); s.public = b.to_string() }
                    ["enabled", v] => s.enabled = *v == "1",
                    ["takeback", v] => s.take_back = *v == "1",
                    ["group", v] => s.group = v.to_string(),
                    ["recvdir", ..] => s.recv_dir = line["recvdir ".len()..].to_string(),
                    ["rev", v] => s.rev = v.parse().unwrap_or(0),
                    ["cell", id, x, y] => s.cells.push((id.to_string(), x.parse().unwrap_or(0), y.parse().unwrap_or(0))),
                    ["peer", id, key, os, name] => s.peers.push(Peer { id: id.to_string(), key: key.to_string(), os: os.parse().unwrap_or(0), name: name.to_string() }),
                    _ => {}
                }
            }
            // 从“两台核对 6 位数字”的旧版升上来的：以前配对的记录用不上了，各台重新加入一次
            if s.group.is_empty() { s.peers.clear(); s.cells.clear(); s.rev = 0 }
            if s.id.len() == 32 && s.private.len() == 64 { return s }
            let _ = std::fs::rename(&file, data_dir().join(format!("share.txt.坏了-{}", std::process::id())));   // 读不懂：留一份，别覆盖
        }
        // 第一次：生成编号和身份
        let mut seed = [0u8; 16];
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let ident = unsafe { let p = ywk_new_identity(); let v = if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }; ywk_free(p); v };
        if let Some((a, b)) = ident.split_once(' ') { s.private = a.into(); s.public = b.into() }
        // 编号：时间 + 进程号 + 公钥的前几位，混一下，够用（只用来区分电脑，不用来保密）
        for (i, b) in seed.iter_mut().enumerate() { *b = ((t >> ((i % 12) * 8)) as u8) ^ u8::from_str_radix(s.public.get(i * 2..i * 2 + 2).unwrap_or("00"), 16).unwrap_or(0) ^ (std::process::id() >> (i % 4 * 8)) as u8 }
        s.id = seed.iter().map(|b| format!("{b:02x}")).collect();
        s.save();
        s
    }
    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        let mut t = format!("id {}\nkey {} {}\nenabled {}\nrev {}\n", self.id, self.private, self.public, self.enabled as u8, self.rev);
        if !self.group.is_empty() { t += &format!("group {}\n", self.group) }
        if !self.recv_dir.is_empty() { t += &format!("recvdir {}\n", self.recv_dir) }
        if self.take_back { t += "takeback 1\n" }
        for (id, x, y) in &self.cells { t += &format!("cell {id} {x} {y}\n") }
        for p in &self.peers { t += &format!("peer {} {} {} {}\n", p.id, if p.key.is_empty() { "-" } else { &p.key }, p.os, p.name.replace('\n', " ")) }
        if self.id.is_empty() { return }
        let tmp = data_dir().join("share.txt.tmp");
        if std::fs::write(&tmp, t).is_ok() { let _ = std::fs::rename(&tmp, data_dir().join("share.txt")); }
    }
}

// MARK: 主线程上的状态

struct Machine { name: String, os: i32, online: bool }
struct Found { id: String, name: String, ip: String, port: u16, joining: bool, seen: std::time::Instant }
/// 正在发、正在收的文件（面板里小图下面显示）。state：0 进行中 1 完成 2 失败
#[derive(Clone)]
pub(crate) struct Job { pub key: String, pub x: u32, pub peer: String, pub sending: bool, pub title: String, pub done: u64, pub total: u64, pub state: u8, pub err: String, pub ended: Option<std::time::Instant> }
struct App { code: String, jobs: Vec<Job>, last_recv: String, balloon_recv: bool, store: Store, machines: HashMap<String, Machine>, found: Vec<Found>, told: HashSet<String>, pending_place: Vec<String>, icon: HICON, problem: Option<String> }
thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
fn app<R>(f: impl FnOnce(&mut App) -> R) -> R { APP.with(|a| f(a.borrow_mut().as_mut().expect("app"))) }

fn running() -> bool { *CORE.read().unwrap() != 0 }
fn with_core(f: impl FnOnce(*mut Handle)) { let g = CORE.read().unwrap(); if *g != 0 { f(*g as *mut Handle) } }
fn cs(s: &str) -> CString { CString::new(s.replace('\0', "")).unwrap() }

// MARK: 屏幕

fn read_screen() {
    unsafe {
        ORIGIN.0.store(GetSystemMetrics(SM_XVIRTUALSCREEN), SeqCst); ORIGIN.1.store(GetSystemMetrics(SM_YVIRTUALSCREEN), SeqCst);
        SIZE.0.store(GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1), SeqCst); SIZE.1.store(GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1), SeqCst);
        PARK.0.store(GetSystemMetrics(SM_CXSCREEN) / 2, SeqCst); PARK.1.store(GetSystemMetrics(SM_CYSCREEN) / 2, SeqCst);
    }
}

// MARK: 藏鼠标：把系统的几种指针都换成透明的；放回来时让系统重新读用户的指针方案

const CURSORS: [u32; 14] = [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650, 32651];
fn hide_cursor() {
    let and = [0xFFu8; 128]; let xor = [0u8; 128];
    unsafe { for id in CURSORS { let c = CreateCursor(GetModuleHandleW(std::ptr::null()), 0, 0, 32, 32, and.as_ptr(), xor.as_ptr()); if c != 0 { SetSystemCursor(c, id); } } }
}
fn show_cursor() { unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), 0); } }

// MARK: 剪贴板（只在主线程上碰）

fn open_clipboard() -> bool {
    for _ in 0..10 { if unsafe { OpenClipboard(MAIN.load(SeqCst)) } != 0 { return true } std::thread::sleep(std::time::Duration::from_millis(15)) }
    false
}
fn clip_get() -> Option<String> {
    unsafe {
        if !open_clipboard() { return None }
        let h = GetClipboardData(CF_UNICODETEXT);
        let mut out = None;
        if h != 0 {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                let cap = GlobalSize(h) / 2;
                let mut n = 0; while n < cap && *p.add(n) != 0 { n += 1 }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}
/// 剪贴板里的图片（系统给的是不带文件头的位图）：补上文件头，就是一个 BMP 文件
fn clip_get_bmp() -> Option<Vec<u8>> {
    unsafe {
        if !open_clipboard() { return None }
        let h = GetClipboardData(CF_DIB);
        let mut out = None;
        if h != 0 {
            let p = GlobalLock(h) as *const u8;
            if !p.is_null() {
                let d = std::slice::from_raw_parts(p, GlobalSize(h));
                out = bmp_from_dib(d);
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
        out
    }
}
/// 位图前面加 14 字节的文件头。像素从哪开始要算上信息头、颜色掩码、调色板
fn bmp_from_dib(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 40 || d.len() > 60 << 20 { return None }
    let u32at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let (size, bits, comp, used) = (u32at(0) as usize, u16::from_le_bytes([d[14], d[15]]) as u32, u32at(16), u32at(32) as usize);
    if size < 40 || size > d.len() || bits > 32 { return None }
    let masks = if size == 40 { match comp { 3 => 12, 6 => 16, _ => 0 } } else { 0 };
    let colors = if used != 0 { used } else if bits <= 8 { 1usize << bits } else { 0 };
    let off = 14 + size + masks + colors.min(256) * 4;
    if off >= 14 + d.len() { return None }
    let mut f = Vec::with_capacity(14 + d.len());
    f.extend_from_slice(b"BM"); f.extend_from_slice(&((14 + d.len()) as u32).to_le_bytes()); f.extend_from_slice(&[0u8; 4]); f.extend_from_slice(&(off as u32).to_le_bytes());
    f.extend_from_slice(d);
    Some(f)
}
// MARK: 剪贴板跟着指针走（规则在核心的 clipflow 里，这里替它读写 Windows 的剪贴板；只在主线程上碰）

/// 整份剪贴板：每种格式一块内存
type Snap = Vec<(u32, Vec<u8>)>;
thread_local! { static FLOW: RefCell<Flow<Snap>> = RefCell::new(Flow::default()); }
fn flow(f: impl FnOnce(&mut Flow<Snap>, &mut WinBoard)) { FLOW.with(|x| f(&mut x.borrow_mut(), &mut WinBoard)) }

/// 一段文字当成一份剪贴板内容
fn text_piece(s: &str) -> Snap {
    let u: Vec<u16> = s.replace('\n', "\r\n").replace("\r\r\n", "\r\n").encode_utf16().chain(std::iter::once(0)).collect();
    vec![(CF_UNICODETEXT, u.iter().flat_map(|c| c.to_le_bytes()).collect())]
}
/// 一个 BMP 文件当成一份剪贴板内容（去掉 14 字节的文件头就是系统要的位图）
fn image_piece(file: &[u8]) -> Option<Snap> {
    if file.len() < 14 + 40 || &file[..2] != b"BM" { return None }
    let d = &file[14..];
    let fmt = if u32::from_le_bytes([d[0], d[1], d[2], d[3]]) >= 108 { CF_DIBV5 } else { CF_DIB };
    Some(vec![(fmt, d.to_vec())])
}
/// 几个文件、文件夹当成一份剪贴板内容（和在资源管理器里复制它们一样）：
/// 20 字节的头（路径表从第 20 字节开始、是宽字符），路径一个接一个各以 0 结尾，最后再多一个 0
fn files_piece(paths: &[String]) -> Snap {
    let mut d = vec![0u8; 20];
    d[0] = 20; d[16] = 1;
    for p in paths { for c in p.encode_utf16().chain(std::iter::once(0)) { d.extend_from_slice(&c.to_le_bytes()) } }
    d.extend_from_slice(&[0, 0]);
    let mut s = vec![(CF_HDROP, d)];
    let effect = unsafe { RegisterClipboardFormatW(w("Preferred DropEffect").as_ptr()) };
    if effect != 0 { s.push((effect, 1u32.to_le_bytes().to_vec())) }       // 粘贴时是“复制”，不是“移动”
    s
}
/// 把剪贴板整份存下来。只存“一块内存”的那些格式：位图句柄、图元文件、调色板系统会从别的格式变出来，
/// 程序私有的句柄存不了。系统能互相变出来的只存一种（有 Unicode 文字就不存另外两种文字；两种位图只存排在前面的那种），
/// 不然一张整屏截图要存两遍。超过 64MB 不存
fn clip_snapshot() -> Option<Snap> {
    unsafe {
        if !open_clipboard() { return None }
        let unicode = IsClipboardFormatAvailable(CF_UNICODETEXT) != 0;
        let (mut f, mut total, mut dib, mut out) = (0u32, 0usize, false, Snap::new());
        loop {
            f = EnumClipboardFormats(f);
            if f == 0 { break }
            if matches!(f, 2 | 3 | 9 | 14 | 0x80..=0x8E | 0x200..=0x3FF) || (unicode && matches!(f, 1 | 7)) { continue }
            if matches!(f, CF_DIB | CF_DIBV5) { if dib { continue } dib = true }
            let h = GetClipboardData(f);
            if h == 0 { continue }
            let p = GlobalLock(h) as *const u8;
            if p.is_null() { continue }
            let n = GlobalSize(h);
            total += n;
            if total > 64 << 20 { GlobalUnlock(h); CloseClipboard(); return None }
            out.push((f, std::slice::from_raw_parts(p, n).to_vec()));
            GlobalUnlock(h);
        }
        CloseClipboard();
        Some(out)
    }
}
/// 整份放进剪贴板，放成了返回 true。guest：这是别的电脑的指针带来的、临时用的，
/// 加上系统认的记号，不让它进本机的剪贴板历史（Win+V）和云剪贴板
fn clip_apply(s: &Snap, guest: bool) -> bool {
    unsafe fn put(f: u32, d: &[u8]) {
        let h = GlobalAlloc(GMEM_MOVEABLE, d.len().max(1));
        if h == 0 { return }
        let p = GlobalLock(h) as *mut u8;
        if !p.is_null() { std::ptr::copy_nonoverlapping(d.as_ptr(), p, d.len()); GlobalUnlock(h); }
        if p.is_null() || SetClipboardData(f, h) == 0 { GlobalFree(h); }       // 放成了这块内存就归系统管，没放成要自己还
    }
    unsafe {
        if !open_clipboard() { return false }
        EmptyClipboard();
        for (f, d) in s { put(*f, d) }
        let mark = RegisterClipboardFormatW(w("ExcludeClipboardContentFromMonitorProcessing").as_ptr());
        if guest && mark != 0 && !s.iter().any(|x| x.0 == mark) { put(mark, &[0; 4]) }
        CloseClipboard();
        true
    }
}
/// 存成临时文件交给核心的图片，各用各的文件名（同一张图可能同时带给两台电脑）
static CLIP_FILES: AtomicU32 = AtomicU32::new(0);
/// 剪贴板第几版时提示过“文件太大没带过去”（同一次复制只提示一回）
static TOO_BIG_AT: AtomicU32 = AtomicU32::new(0);
struct WinBoard;
impl Board for WinBoard {
    type Snap = Snap;
    fn version(&mut self) -> u32 { unsafe { GetClipboardSequenceNumber() } }
    fn snapshot(&mut self) -> Option<Snap> { clip_snapshot() }
    fn apply(&mut self, s: &Snap, guest: bool) -> bool { clip_apply(s, guest) }
    /// 复制的是文件、文件夹：合起来 100MB 以内就带（原件不动），太大的不带、提示改用“发送文件”。
    /// 不然带文字、图片；两样都有（比如从表格里复制）就都带：对面粘贴到哪种程序里都对
    fn carry(&mut self, id: &str, tag: ClipTag) -> bool {
        let (id, back, mut any) = (cs(id), tag.back as i32, false);
        if unsafe { IsClipboardFormatAvailable(CF_HDROP) } != 0 {
            let paths: Vec<String> = panel::clipboard_files(MAIN.load(SeqCst)).into_iter().filter(|p| !p.contains('\n')).collect();
            if paths.is_empty() { return false }
            let (list, seq, mut r) = (cs(&paths.join("\n")), unsafe { GetClipboardSequenceNumber() }, -1);
            with_core(|h| r = ywk_clip_files(h, id.as_ptr(), list.as_ptr(), back, tag.no));
            if r == 0 && TOO_BIG_AT.swap(seq, SeqCst) != seq {
                app(|a| a.balloon_recv = false);                       // 点这条提示不该去打开上次收到的文件
                tray(NIM_MODIFY, Some(("复制的文件超过 100MB（或多于 2000 个），没有带过去", "在一苇的小图上右键那台电脑，选“发送文件”")));
            }
            return r == 1;
        }
        let (text, image) = (clip_get(), clip_get_bmp());
        if let Some(t) = text.filter(|t| !t.is_empty() && t.len() <= 4 << 20) {
            let t = cs(&t.replace("\r\n", "\n"));
            with_core(|h| ywk_clip(h, id.as_ptr(), t.as_ptr(), back, tag.no));
            any = true;
        }
        if let Some(bmp) = image {
            // 存成临时文件交给核心（发完核心会删）
            let path = std::env::temp_dir().join(format!("yiwei-clip-{}-{}.bmp", std::process::id(), CLIP_FILES.fetch_add(1, SeqCst)));
            if std::fs::write(&path, bmp).is_ok() {
                let p = cs(&path.to_string_lossy());
                with_core(|h| ywk_clip_image(h, id.as_ptr(), p.as_ptr(), back, tag.no));
                any = true;
            }
        }
        any
    }
    fn carry_nothing(&mut self, id: &str, tag: ClipTag) { let (id, e) = (cs(id), cs("")); with_core(|h| ywk_clip(h, id.as_ptr(), e.as_ptr(), 0, tag.no)) }
    fn join(&mut self, a: &Snap, b: &Snap) -> Snap { a.iter().chain(b).cloned().collect() }
    fn note(&mut self, line: String) { slog(&line) }
}

// MARK: 核心的回调（在核心的线程上）

extern "C" fn on_notice(_: *mut c_void, j: *const c_char) {
    let s = unsafe { CStr::from_ptr(j) }.to_string_lossy().into_owned();
    if s.starts_with(r#"{"t":"grab""#) {
        REMOTE.store(true, SeqCst);
        // 穿到哪台上了也带给主线程：剪贴板要带给它
        let b = Box::into_raw(Box::new(json::parse(&s).map(|j| j.str("id")).unwrap_or_default()));
        unsafe { SetCursorPos(PARK.0.load(SeqCst), PARK.1.load(SeqCst)); if PostMessageW(MAIN.load(SeqCst), WM_GRABBED, 0, b as isize) == 0 { drop(Box::from_raw(b)) } }
    } else if s.starts_with(r#"{"t":"release""#) {
        let j = json::parse(&s).unwrap_or(json::J::Null);
        unsafe { SetCursorPos(ORIGIN.0.load(SeqCst) + j.num("x") as i32, ORIGIN.1.load(SeqCst) + j.num("y") as i32); }
        REMOTE.store(false, SeqCst);
        unsafe { PostMessageW(MAIN.load(SeqCst), WM_RELEASED, 0, 0); }
    } else {
        let b = Box::into_raw(Box::new(s));
        if unsafe { PostMessageW(MAIN.load(SeqCst), WM_NOTICE, 0, b as isize) } == 0 { drop(unsafe { Box::from_raw(b) }) }
    }
}

/// 注入别的电脑发来的输入。记着按下了什么，对方走了（或掉线了）就全部松开
#[derive(Default)]
struct Held { keys: HashSet<u16>, buttons: HashSet<i32> }
static HELD: Mutex<Option<Held>> = Mutex::new(None);

fn send(inputs: &[INPUT]) { unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); } }
fn mouse(flags: u32, dx: i32, dy: i32, data: u32) -> INPUT { INPUT { kind: INPUT_MOUSE, u: INPUT_U { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: MARK } } } }
fn move_to(x: i32, y: i32) {
    let (w, h) = (SIZE.0.load(SeqCst), SIZE.1.load(SeqCst));
    let nx = (x.clamp(0, w - 1) as i64 * 65535 / (w - 1).max(1) as i64) as i32;
    let ny = (y.clamp(0, h - 1) as i64 * 65535 / (h - 1).max(1) as i64) as i32;
    send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)]);
}
fn key(held: &mut Held, usage: u16, down: bool) {
    let Some((scan, ext)) = win_from_usage(usage) else { return };
    if down { held.keys.insert(usage); } else { held.keys.remove(&usage); }
    let flags = KEYEVENTF_SCANCODE | if ext { KEYEVENTF_EXTENDEDKEY } else { 0 } | if down { 0 } else { KEYEVENTF_KEYUP };
    send(&[INPUT { kind: INPUT_KEYBOARD, u: INPUT_U { ki: KEYBDINPUT { wVk: 0, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MARK } } }]);
}
fn button(held: &mut Held, b: i32, down: bool) {
    let (flag, data) = match (b, down) {
        (1, true) => (MOUSEEVENTF_LEFTDOWN, 0), (1, false) => (MOUSEEVENTF_LEFTUP, 0),
        (2, true) => (MOUSEEVENTF_RIGHTDOWN, 0), (2, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (3, true) => (MOUSEEVENTF_MIDDLEDOWN, 0), (3, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (4 | 5, true) => (MOUSEEVENTF_XDOWN, (b - 3) as u32), (4 | 5, false) => (MOUSEEVENTF_XUP, (b - 3) as u32),
        _ => return,
    };
    if down { held.buttons.insert(b); } else { held.buttons.remove(&b); }
    send(&[mouse(flag, 0, 0, data)]);
}
fn release_all(held: &mut Held) {
    if held.keys.contains(&0xE3) || held.keys.contains(&0xE7) {          // 单独按下又松开 Win 会弹开始菜单：中间插一个不存在的键
        let k = |up: u32| INPUT { kind: INPUT_KEYBOARD, u: INPUT_U { ki: KEYBDINPUT { wVk: 0xFF, wScan: 0, dwFlags: up, time: 0, dwExtraInfo: MARK } } };
        send(&[k(0), k(KEYEVENTF_KEYUP)]);
    }
    for u in std::mem::take(&mut held.keys) { key(held, u, false) }
    held.keys.clear();
    for b in std::mem::take(&mut held.buttons) { button(held, b, false) }
    held.buttons.clear();
}
extern "C" fn on_input(_: *mut c_void, kind: i32, a: i32, b: i32, c: i32, d: i32) {
    let mut g = HELD.lock().unwrap();
    let held = g.get_or_insert_with(Held::default);
    match kind {
        1 => move_to(a, b),
        2 => {
            if b != 0 { send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, b as u32)]) }     // 120 = 一格，正数往上
            if a != 0 { send(&[mouse(MOUSEEVENTF_HWHEEL, 0, 0, a as u32)]) }    // 正数往右
        }
        3 => key(held, a as u16, b == 1),
        4 => { move_to(c, d); button(held, a, b == 1) }
        5 => {
            BOSSED.store(true, SeqCst);
            if IDLE_HIDDEN.swap(false, SeqCst) { unsafe { PostMessageW(MAIN.load(SeqCst), WM_SHOW_LOCAL, 0, 0); } }
            for (bit, u) in [(1, 0xE1u16), (2, 0xE0), (4, 0xE2), (8, 0xE3)] { if c & bit != 0 && !held.keys.contains(&u) { key(held, u, true) } }
            // 大写锁定跟着对方的键盘：交给主线程去看本机现在的状态，不一样就按一下
            unsafe { PostMessageW(MAIN.load(SeqCst), WM_SET_CAPS, (c & 16 != 0) as usize, 0); PostMessageW(MAIN.load(SeqCst), WM_PEER_CAME, 0, 0); }
        }
        // a：0 正常走了（本机指针藏起来放中间）、1 被请回去了（换了一台来控制，或者本机的鼠标自己穿出去了）、2 对方掉线了
        6 => { BOSSED.store(false, SeqCst); release_all(held); unsafe { PostMessageW(MAIN.load(SeqCst), WM_PEER_LEFT, a as usize, 0); } }
        _ => {}
    }
}

// MARK: 监听本机键鼠（单独一个线程，钩子要它的消息循环转着）

thread_local! {
    static ESC: RefCell<(Vec<std::time::Instant>, bool)> = const { RefCell::new((Vec::new(), false)) };
    static IDLE_MOVED: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    /// 别的程序模拟的、给了本机的按键（按虚拟键码记）：抬起也给本机
    static INJ_DOWN: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
}

/// 系统说现在真按着的修饰键：第 0–7 位依次是左 Ctrl、左 Shift、左 Alt、左 Win、右 Ctrl、右 Shift、右 Alt、右 Win
unsafe fn held_mods() -> u32 {
    const VK: [i32; 8] = [0xA2, 0xA0, 0xA4, 0x5B, 0xA3, 0xA1, 0xA5, 0x5C];
    let mut m = 0;
    for (i, vk) in VK.iter().enumerate() { if GetAsyncKeyState(*vk) as u16 & 0x8000 != 0 { m |= 1 << i } }
    m
}

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code < 0 { return CallNextHookEx(0, code, wp, lp) }
    let m = &*(lp as *const MSLLHOOKSTRUCT);
    let msg = wp as u32;
    if m.dwExtraInfo == MARK { return CallNextHookEx(0, code, wp, lp) }      // 自己注入的对方的输入
    let g = CORE.read().unwrap();
    let h = *g as *mut Handle;
    if h.is_null() { drop(g); return CallNextHookEx(0, code, wp, lp) }
    let remote = ywk_remote(h) == 1;
    let press = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL);
    // 不是本机直接接的鼠标发出来的：移动（自己挪指针引起的、别的程序模拟的）一律不报；
    // 点击和滚轮，鼠标在本机时不报（本来就是给本机的），鼠标在别的电脑上时照常发过去
    if m.flags & LLMHF_INJECTED != 0 && (msg == WM_MOUSEMOVE || !remote) {
        if press && IDLE_HIDDEN.load(SeqCst) { show_idle() }                 // 有人用远程软件在点这台电脑：藏着的指针显示出来
        drop(g);
        return CallNextHookEx(0, code, wp, lp);
    }
    // 这次真的移动了多少：想去的位置减去指针现在的位置（顶着边推时，想去的位置在屏幕外面）
    let mut cur = POINT::default(); GetCursorPos(&mut cur);
    let (mut dx, mut dy) = if msg == WM_MOUSEMOVE { (m.pt.x - cur.x, m.pt.y - cur.y) } else { (0, 0) };
    // 对方的鼠标走了以后本机指针藏着：本机鼠标真的动了（不是轻轻碰一下）就显示出来
    if !remote && IDLE_HIDDEN.load(SeqCst) {
        let real = press || (msg == WM_MOUSEMOVE && IDLE_MOVED.with(|c| { c.set(c.get() + dx.abs() + dy.abs()); c.get() >= 6 }));
        if real { show_idle() }
    } else { IDLE_MOVED.with(|c| c.set(0)) }
    let taken = match msg {
        WM_MOUSEMOVE => {
            if remote {
                // 鼠标停在主屏中间，移动量照上面的算。现在不在停靠点（刚穿走还没停好、锁屏、别的程序挪过它、换了分辨率）
                // 就拉回去，这一下不算
                let park = (PARK.0.load(SeqCst), PARK.1.load(SeqCst));
                let off = (cur.x, cur.y) != park;
                // 第五个数：这个事件从鼠标发出到钩子被调用隔了多少毫秒（大了说明是系统那边送得晚，不是鼠标没发）
                ywk_trace(4, dx as f64, dy as f64, (cur.x - park.0) as f64, (cur.y - park.1) as f64, GetTickCount().wrapping_sub(m.time) as i32 as f64, off as i32 as f64);
                if off { SetCursorPos(park.0, park.1); } else { ywk_remote_move(h, dx as f64, dy as f64); }
                true
            } else {
                let (w, hh) = (SIZE.0.load(SeqCst), SIZE.1.load(SeqCst));
                let (x, y) = ((m.pt.x - ORIGIN.0.load(SeqCst)).clamp(0, w - 1), (m.pt.y - ORIGIN.1.load(SeqCst)).clamp(0, hh - 1));
                let edge = x <= 0 || y <= 0 || x >= w - 1 || y >= hh - 1;
                // 顶在边上还往外推，系统给的位置却没变（两个方向都没动）：贴着哪条边就当作往哪边推。
                // 沿着边滑（另一个方向在动）不算往外推，不会滑一下就穿过去
                if dx == 0 && dy == 0 {
                    if x <= 0 { dx = -1 } else if x >= w - 1 { dx = 1 }
                    if y <= 0 { dy = -1 } else if y >= hh - 1 { dy = 1 }
                }
                // 贴边了，可能要穿屏：先按系统说的对一下修饰键（抬起没监听到的补上，免得带着它穿过去）
                if edge { ywk_sync_mods(h, held_mods() | if GetKeyState(0x14) & 1 != 0 { 0x100 } else { 0 }) }
                ywk_local_move(h, x as f64, y as f64, dx as f64, dy as f64);
                false
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => ywk_button(h, 1, (msg == WM_LBUTTONDOWN) as i32) == 1,
        WM_RBUTTONDOWN | WM_RBUTTONUP => ywk_button(h, 2, (msg == WM_RBUTTONDOWN) as i32) == 1,
        WM_MBUTTONDOWN | WM_MBUTTONUP => ywk_button(h, 3, (msg == WM_MBUTTONDOWN) as i32) == 1,
        WM_XBUTTONDOWN | WM_XBUTTONUP => ywk_button(h, 3 + (m.mouseData >> 16) as i32, (msg == WM_XBUTTONDOWN) as i32) == 1,
        WM_MOUSEWHEEL => ywk_wheel(h, 0, (m.mouseData >> 16) as u16 as i16 as i32) == 1,
        WM_MOUSEHWHEEL => ywk_wheel(h, (m.mouseData >> 16) as u16 as i16 as i32, 0) == 1,
        _ => false,
    };
    drop(g);
    if taken { 1 } else { CallNextHookEx(0, code, wp, lp) }
}

unsafe extern "system" fn key_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code < 0 { return CallNextHookEx(0, code, wp, lp) }
    let k = &*(lp as *const KBDLLHOOKSTRUCT);
    if k.dwExtraInfo == MARK { return CallNextHookEx(0, code, wp, lp) }
    let down = matches!(wp as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
    let g = CORE.read().unwrap();
    let h = *g as *mut Handle;
    if h.is_null() { drop(g); return CallNextHookEx(0, code, wp, lp) }
    let remote = ywk_remote(h) == 1;
    // 别的程序模拟的按键（改键软件、远程软件，不是本机键盘按的）：鼠标在本机时不报给核心，直接给本机。
    // 记下它：它的抬起即使鼠标穿走以后才来，也还是给本机，不会在本机一直按着
    if k.flags & LLKHF_INJECTED != 0 {
        let local = INJ_DOWN.with(|s| {
            let mut s = s.borrow_mut();
            if !down { s.remove(&k.vkCode) } else if !remote || s.contains(&k.vkCode) { s.insert(k.vkCode); true } else { false }
        });
        if local {
            if down && IDLE_HIDDEN.load(SeqCst) { show_idle() }
            drop(g);
            return CallNextHookEx(0, code, wp, lp);
        }
    }
    if down && !remote && IDLE_HIDDEN.load(SeqCst) { show_idle() }
    // 连按三下 Esc：鼠标马上回到本机
    if k.vkCode == 0x1B {
        let home = ESC.with(|e| {
            let mut e = e.borrow_mut();
            if !down { e.1 = false; return false }                            // 抬起：不管鼠标在哪都记下（第三下的抬起来的时候鼠标已经回来了）
            if !remote || e.1 { return false }                                // 按住不放的重复不算
            e.1 = true;
            let now = std::time::Instant::now();
            e.0.retain(|t| now.duration_since(*t).as_millis() < 800); e.0.push(now);
            if e.0.len() >= 3 { e.0.clear(); true } else { false }
        });
        if home { ywk_home(h); drop(g); return 1 }
    }
    // 不发的：小键盘的“假 Shift”（0x2A 带扩展位）、AltGr 布局伪造的左 Ctrl（扫描码带 0x200）、Pause、程序注入的 Unicode 字符
    let fake_shift = (k.scanCode == 0x2A && k.flags & LLKHF_EXTENDED != 0) || k.scanCode & 0x200 != 0 || k.vkCode == 0x13 || k.vkCode == 0xE7;
    let ext = k.flags & LLKHF_EXTENDED != 0 && k.vkCode != 0x90;                 // 数字锁定键报的是扩展位，表里不是
    let taken = match (fake_shift, usage_from_win(k.scanCode as u16, ext)) {
        // 大写锁定：发给对面，同时也留给本机——本机键盘上的灯和对面的大小写一起变，不会一边大写一边小写
        (false, Some(u)) => ywk_key(h, u as i32, down as i32) == 1 && k.vkCode != 0x14,
        _ => false,                                                   // 媒体键之类不认识的：留给本机
    };
    drop(g);
    if taken { 1 } else { CallNextHookEx(0, code, wp, lp) }
}

/// 本机指针藏着等本机键鼠动：显示出来
unsafe fn show_idle() {
    if IDLE_HIDDEN.swap(false, SeqCst) { PostMessageW(MAIN.load(SeqCst), WM_SHOW_LOCAL, 0, 0); }
}

fn start_hooks() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        yiwei_core::urgent_thread();
        let inst = GetModuleHandleW(std::ptr::null());
        let m = SetWindowsHookExW(WH_MOUSE_LL, mouse_hook, inst, 0);
        let k = SetWindowsHookExW(WH_KEYBOARD_LL, key_hook, inst, 0);
        let _ = tx.send((GetCurrentThreadId(), m != 0 && k != 0));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 { TranslateMessage(&msg); DispatchMessageW(&msg); }
        if m != 0 { UnhookWindowsHookEx(m); }
        if k != 0 { UnhookWindowsHookEx(k); }
    });
    if let Ok((tid, ok)) = rx.recv() {
        HOOK_THREAD.store(tid, SeqCst);
        slog(if ok { "键鼠钩子已装上" } else { "键鼠钩子装不上" });
        if !ok { app(|a| a.problem = Some("没能监听键鼠".into())) }
    }
}
fn stop_hooks() {
    let t = HOOK_THREAD.swap(0, SeqCst);
    if t != 0 { unsafe { PostThreadMessageW(t, WM_QUIT, 0, 0); } }
}

// MARK: 开关

fn start_share() {
    if running() { return }
    if app(|a| a.store.id.is_empty()) { app(|a| a.problem = Some("读不了存档，请重新打开一苇".into())); return }
    read_screen();
    let (s, group) = app(|a| (a.store.id.clone() + " " + &a.store.private + " " + &a.store.public, a.store.group.clone()));
    let parts: Vec<&str> = s.split(' ').collect();
    let name = std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "Windows 电脑".into());
    let (id, private, public, name, group) = (cs(parts[0]), cs(parts[1]), cs(parts[2]), cs(&name), cs(&group));
    { let lp = cs(&log_path()); ywk_set_log(lp.as_ptr()) }
    slog(&format!("Windows 启动键鼠共享：虚拟屏幕 {}×{} 原点 ({}, {})，停靠点 ({}, {})", SIZE.0.load(SeqCst), SIZE.1.load(SeqCst), ORIGIN.0.load(SeqCst), ORIGIN.1.load(SeqCst), PARK.0.load(SeqCst), PARK.1.load(SeqCst)));
    let h = ywk_start(id.as_ptr(), name.as_ptr(), 2, SIZE.0.load(SeqCst) as u32, SIZE.1.load(SeqCst) as u32,
                      private.as_ptr(), public.as_ptr(), group.as_ptr(), 0, 1, Some(on_notice), Some(on_input), std::ptr::null_mut());
    if h.is_null() {
        app(|a| a.problem = Some("端口 47800 被占用".into()));
        return;
    }
    let code = unsafe { let p = ywk_code(h); let v = if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }; ywk_free(p); v };
    app(|a| a.code = code);
    let (rev, cells) = app(|a| (a.store.rev, a.store.cells.iter().map(|(i, x, y)| format!("{i}:{x}:{y}")).collect::<Vec<_>>().join(",")));
    if !cells.is_empty() { let c = cs(&cells); ywk_restore_layout(h, rev, c.as_ptr()) }
    { let d = cs(&recv_dir()); ywk_set_recv_dir(h, d.as_ptr()) }
    ywk_set_take_back(h, app(|a| a.store.take_back) as i32);
    *CORE.write().unwrap() = h as usize;
    app(|a| a.problem = None);
    start_hooks();
}
/// 不管鼠标在哪台电脑上，马上回到本机，指针显示出来
fn go_home() {
    with_core(|c| ywk_home(c));
    if REMOTE.swap(false, SeqCst) { unsafe { SetCursorPos(PARK.0.load(SeqCst), PARK.1.load(SeqCst)); } }
    show_cursor();
}
fn stop_share() {
    let h = std::mem::take(&mut *CORE.write().unwrap());      // 先拿掉核心：之后钩子里不会再把鼠标“穿走”
    stop_hooks();
    REMOTE.store(false, SeqCst);
    show_cursor();
    if h != 0 { ywk_stop(h as *mut Handle) }
    REMOTE.store(false, SeqCst); BOSSED.store(false, SeqCst);   // 核心收尾时可能又说了一次“穿走 / 回来”：最后以本机为准
    show_cursor();
    if let Some(held) = HELD.lock().unwrap().as_mut() { release_all(held) }
    flow(|f, b| f.reset(b));                                    // 客人的那份还在剪贴板里的话，恢复成本机自己的
    app(|a| { for m in a.machines.values_mut() { m.online = false } a.found.clear(); });
}

// MARK: 开机自启（当前用户的“启动”注册表项）

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
fn autostart() -> bool {
    let mut n = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, w(RUN_KEY).as_ptr(), w("Yiwei").as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut n) == 0 }
}
fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| format!("\"{}\"", p.display())).unwrap_or_default();
            let v = w(&exe);
            RegSetKeyValueW(HKEY_CURRENT_USER, w(RUN_KEY).as_ptr(), w("Yiwei").as_ptr(), REG_SZ, v.as_ptr() as *const c_void, (v.len() * 2) as u32);
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, w(RUN_KEY).as_ptr(), w("Yiwei").as_ptr());
        }
    }
}

// MARK: 托盘

fn tray(op: u32, info: Option<(&str, &str)>) {
    let (icon, tip) = app(|a| {
        let n = a.machines.values().filter(|m| m.online).count();
        let tip = if !running() { a.problem.clone().unwrap_or_else(|| "一苇：已关闭".into()) }
                  else if n > 0 { format!("一苇：已连接 {n} 台") } else { "一苇：未连接".into() };
        (a.icon, tip)
    });
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = MAIN.load(SeqCst); d.uID = 1; d.uCallbackMessage = WM_TRAY; d.hIcon = icon;
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    fill(&mut d.szTip, &tip);
    if let Some((title, text)) = info { d.uFlags |= NIF_INFO; d.dwInfoFlags = NIIF_INFO; fill(&mut d.szInfoTitle, title); fill(&mut d.szInfo, text) }
    unsafe { Shell_NotifyIconW(op, &d); }
}

fn local_ip() -> Option<String> {
    // 不发包：只借系统的路由表问“去外面走哪块网卡”
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// 加入发现的一台电脑所在的组。code 是用户输入的、它的校验码
fn join(id: &str, code: &str) {
    let Some((ip, port)) = app(|a| a.found.iter_mut().find(|f| f.id == id).map(|f| { f.joining = true; f.seen = std::time::Instant::now(); (f.ip.clone(), f.port) })) else { return };
    let (ip, code) = (cs(&ip), cs(code));
    with_core(|h| ywk_join(h, ip.as_ptr(), port, code.as_ptr()));
}

/// 有电脑输对了校验码想加入：在单独的线程里弹出来问，免得挡住托盘
fn ask_join(id: String, name: String) {
    static ASKING: Mutex<Vec<String>> = Mutex::new(Vec::new());
    { let mut a = ASKING.lock().unwrap(); if a.contains(&id) || a.len() >= 3 { return } a.push(id.clone()); }
    std::thread::spawn(move || {
        let text = "它输对了校验码。\n允许后，它和组里每台电脑都能互相控制、互发文件。\n\n允许吗？";
        let r = unsafe { MessageBoxW(0, w(text).as_ptr(), w(&format!("一苇 · “{name}”想加入这一组")).as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST) };
        let c = cs(&id);
        with_core(|h| ywk_confirm(h, c.as_ptr(), (r == IDYES) as i32));
        ASKING.lock().unwrap().retain(|x| x != &id);
    });
}

/// 新连上一台电脑后，两台里谁还没有位置就给谁找一个（编号小的那台来做）。和 Mac 版的做法一样
fn place_if_needed(id: &str) {
    let (me, cells, online) = app(|a| (a.store.id.clone(), a.store.cells.clone(), a.machines.get(id).map_or(false, |m| m.online)));
    if !running() || !online || me.as_str() >= id { return }
    let mut taken: Vec<(i32, i32)> = cells.iter().map(|c| (c.1, c.2)).collect();
    fn spot(taken: &mut Vec<(i32, i32)>, a: (i32, i32)) -> (i32, i32) {
        let p = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().map(|d| (a.0 + d.0, a.1 + d.1)).find(|p| !taken.contains(p))
            .unwrap_or((taken.iter().map(|t| t.0).max().unwrap_or(0) + 1, a.1));
        taken.push(p); p
    }
    let mine = cells.iter().find(|c| c.0 == me).map(|c| (c.1, c.2));
    let theirs = cells.iter().find(|c| c.0 == id).map(|c| (c.1, c.2));
    let moves: Vec<(String, (i32, i32))> = match (mine, theirs) {
        (None, None) => { taken.push((0, 0)); vec![(me.clone(), (0, 0)), (id.to_string(), spot(&mut taken, (0, 0)))] }
        (None, Some(t)) => vec![(me.clone(), spot(&mut taken, t))],
        (Some(m), None) => vec![(id.to_string(), spot(&mut taken, m))],
        _ => return,
    };
    for (who, p) in moves { let c = cs(&who); with_core(|h| ywk_place(h, c.as_ptr(), p.0, p.1)) }
}

fn handle_notice(s: &str) {
    let Some(j) = json::parse(s) else { return };
    let (t, id, name, os) = (j.str("t"), j.str("id"), j.str("name"), j.num("os") as i32);
    let mut balloon: Option<(String, String)> = None;
    match t.as_str() {
        "found" => app(|a| {
            if a.machines.get(&id).map_or(false, |m| m.online) { return }
            let (ip, port) = (j.str("ip"), j.num("port") as u16);
            if let Some(f) = a.found.iter_mut().find(|f| f.id == id) { f.ip = ip; f.port = port; f.name = name.clone(); f.seen = std::time::Instant::now() }
            else { a.found.push(Found { id: id.clone(), name: name.clone(), ip, port, joining: false, seen: std::time::Instant::now() }) }
            if a.told.insert(id.clone()) { balloon = Some((format!("发现“{name}”"), "点一苇图标加入".into())) }
        }),
        // 本机所在的组变了：存下来。加入了别人的组的话，原来组里的电脑和摆法都不要了
        "group" => app(|a| {
            a.code = j.str("code"); a.store.group = j.str("save");
            if j.num("joined") as i32 == 1 { a.store.peers.clear(); a.store.cells.clear(); a.store.rev = 0; a.machines.clear(); a.pending_place.clear() }
            a.store.save();
        }),
        "ask" => ask_join(id, name),
        "join_failed" => app(|a| {
            for f in a.found.iter_mut() { f.joining = false }
            let who = if name.is_empty() { "那台电脑".to_string() } else { format!("“{name}”") };
            balloon = Some(match j.num("why") as i32 {
                1 => ("校验码不对".into(), format!("校验码在{who}的面板里，点“展开”能看到")),
                2 => (format!("{who}没有同意"), "没有加入".into()),
                3 => (format!("{who}暂时不接受加入"), "校验码连着输错了几次，过一分钟再试".into()),
                _ => (format!("没连上{who}"), "看看它是不是开着键鼠共享、和这台在同一个网络里".into()),
            });
        }),
        // 两边的一苇不是同一版，连不上：告诉用户该更新哪一台（面板里一直显示，气泡只在第一次弹）
        "mismatch" => app(|a| {
            let text = if j.num("theirs") < j.num("mine") { format!("“{name}”上的一苇是旧版，请更新它") } else { format!("“{name}”上的一苇比这台新，请更新这台") };
            if a.problem.as_deref() != Some(text.as_str()) { balloon = Some(("两台电脑上的一苇不是同一版".into(), text.clone())) }
            a.problem = Some(text);
        }),
        // 发过去的键鼠对方一直没确认，核心已经断开它重连，鼠标回本机了
        "stalled" => app(|a| {
            let text = format!("“{name}”没有响应（它的防火墙可能挡住了一苇）");
            if a.problem.as_deref() != Some(text.as_str()) { balloon = Some((format!("“{name}”没有响应，鼠标已回到这台"), "检查那台电脑的防火墙有没有放行一苇".into())) }
            a.problem = Some(text);
        }),
        "connected" => {
            app(|a| {
                if a.problem.as_deref().map_or(false, |p| p.contains("请更新")) { a.problem = None }
                let first = !a.machines.get(&id).map_or(false, |m| m.online);
                a.machines.insert(id.clone(), Machine { name: name.clone(), os, online: true });
                a.found.retain(|f| f.id != id);
                // 记下组里有这台（没开机时在位置图上是灰的）
                if let Some(p) = a.store.peers.iter_mut().find(|p| p.id == id) { if p.name != name || p.os != os { p.name = name.clone(); p.os = os; a.store.save() } }
                else { a.store.peers.push(Peer { id: id.clone(), key: String::new(), os, name: name.clone() }); a.store.save() }
                if first { balloon = Some((format!("已连上“{name}”"), "鼠标移出屏幕边缘切换，连按三下 Esc 回来".into())) }
                a.pending_place.push(id.clone());
            });
            unsafe { SetTimer(MAIN.load(SeqCst), TIMER_PLACE, 1000, std::ptr::null()); }      // 等对方把它存的摆法发过来再看
        }
        "disconnected" => { app(|a| { if let Some(m) = a.machines.get_mut(&id) { m.online = false } a.found.retain(|f| f.id != id) }); flow(|f, _| f.disconnected(&id)) }
        "guest" => flow(|f, _| f.guest_coming(&id)),
        "clip" => {
            let (text, tag) = (j.str("text"), ClipTag { back: j.num("back") == 1.0, no: j.num("no") as u32 });
            if !text.is_empty() { flow(|f, b| f.arrived(b, &id, tag, false, text_piece(&text))) }
            else if !tag.back { flow(|f, b| f.cleared(b, &id, tag.no)) }
        }
        "clip_failed" => flow(|f, _| f.forget_sent(&id)),              // 图片、文件半路没带成：下次指针过去重新带
        // 对方带来的复制的文件、文件夹（核心已经收在临时文件夹里）：当成“复制了这几个文件”
        "clip_files" => {
            let paths: Vec<String> = j.get("paths").map_or(Vec::new(), |p| p.arr().iter().map(|x| x.as_str().to_string()).collect());
            let tag = ClipTag { back: j.num("back") == 1.0, no: j.num("no") as u32 };
            if !paths.is_empty() { flow(|f, b| f.arrived(b, &id, tag, false, files_piece(&paths))) }
        }
        // 对方带来的图片（核心已经存成文件）
        "clip_image" => {
            let path = j.str("path");
            let tag = ClipTag { back: j.num("back") == 1.0, no: j.num("no") as u32 };
            if let Some(piece) = std::fs::read(&path).ok().and_then(|file| image_piece(&file)) { flow(|f, b| f.arrived(b, &id, tag, true, piece)) }
            let _ = std::fs::remove_file(&path);
        }
        "send_progress" | "recv_progress" => app(|a| {
            let key = format!("{}{}", if t == "send_progress" { "s".to_string() } else { format!("r{id}") }, j.num("x") as u32);
            if let Some(b) = a.jobs.iter_mut().find(|b| b.key == key) { b.done = j.num("done") as u64; b.total = j.num("total") as u64 }
        }),
        "recv_begin" => { unsafe { SetTimer(MAIN.load(SeqCst), TIMER_JOBS, 500, std::ptr::null()); } app(|a| {
            let (x, count) = (j.num("x") as u32, j.num("count") as usize);
            let who = a.machines.get(&id).map_or("另一台电脑".to_string(), |m| m.name.clone());
            let what = format!("{}{}", j.str("name"), if count > 1 { format!(" 等 {count} 个") } else { String::new() });
            a.jobs.push(Job { key: format!("r{id}{x}"), x, peer: id.clone(), sending: false, title: format!("收{who}：{what}"), done: 0, total: j.num("total") as u64, state: 0, err: String::new(), ended: None });
        }) }
        "send_done" | "recv_done" => {
            let ok = matches!(j.get("ok"), Some(json::J::Bool(true)));
            let key = format!("{}{}", if t == "send_done" { "s".to_string() } else { format!("r{id}") }, j.num("x") as u32);
            app(|a| if let Some(b) = a.jobs.iter_mut().find(|b| b.key == key) {
                b.state = if ok { 1 } else { 2 }; b.err = j.str("err"); b.ended = Some(std::time::Instant::now());
                if ok { b.done = b.total }
            });
            if t == "recv_done" && ok {
                let (first, count, total) = (j.str("first"), j.num("count") as usize, j.num("total") as u64);
                let who = app(|a| { a.last_recv = first.clone(); a.machines.get(&id).map_or("另一台电脑".to_string(), |m| m.name.clone()) });
                let name = std::path::Path::new(&first).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                balloon = Some((format!("收到“{who}”发来的 {count} 个文件"), format!("{name}{}，共 {}，点这里打开", if count > 1 { " 等" } else { "" }, size_text(total))));
            }
        }
        "layout" => app(|a| {
            a.store.rev = j.num("rev") as u64;
            a.store.cells = j.get("cells").map(|c| c.arr().iter().map(|e| { let e = e.arr(); (e.first().map_or("", |v| v.as_str()).to_string(), e.get(1).map_or(0.0, |v| v.as_num()) as i32, e.get(2).map_or(0.0, |v| v.as_num()) as i32) }).collect()).unwrap_or_default();
            a.store.save();
        }),
        _ => return,
    }
    // 记住最后弹的是不是“收到文件”：点它才打开文件夹
    if balloon.is_some() { let recv = t == "recv_done"; app(|a| a.balloon_recv = recv) }
    tray(NIM_MODIFY, balloon.as_ref().map(|(a, b)| (a.as_str(), b.as_str())));
    panel::refresh();
}
// MARK: 换新版。Yiwei.exe 这个位置上的文件变了（放了新版；正在运行的旧文件是被改了名的），就自己退出、把新的启动起来，
// 不用人去托盘里点“退出”再双击。每 10 秒看一眼，连着两次看到同一个新文件才换（免得拿到写了一半的）
thread_local! { static UPDATE: RefCell<(Option<(u64, std::time::SystemTime)>, Option<(u64, std::time::SystemTime)>)> = const { RefCell::new((None, None)) }; }
fn exe_stamp() -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(std::env::current_exe().ok()?.with_file_name("Yiwei.exe")).ok()?;
    Some((m.len(), m.modified().ok()?))
}
fn check_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::fs::remove_file(exe.with_file_name("Yiwei-旧版.exe"));        // 上一版退出以后留下的
    let now = exe_stamp();
    let go = UPDATE.with(|u| {
        let mut u = u.borrow_mut();
        if now.is_none() || now == u.0 { u.1 = None; return false }
        if u.1 == now { return true }
        u.1 = now; false
    });
    if !go { return }
    slog("发现新版的 Yiwei.exe，换过去");
    stop_share();
    tray(NIM_DELETE, None);
    // 不能自己直接启动新的：从共享文件夹（网络路径）运行时，本进程还开着旧文件，系统会把它当成“Yiwei.exe”接着用，
    // 启动起来的还是旧版（2026-11-13 真机：换过去以后记录里还是旧的协议版本）。让系统的命令行等本进程退干净了再启动
    use std::os::windows::process::CommandExt;
    let path = exe.with_file_name("Yiwei.exe");
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/c \"ping -n 6 127.0.0.1 >nul & start \"\" \"{}\"\"", path.display()))
        .creation_flags(0x0800_0000)                                         // 不弹黑窗口
        .spawn();
    std::process::exit(0);
}

fn win_rect(h: HWND, r: &mut RECT) { unsafe { GetWindowRect(h, r); } }
fn size_text(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 { format!("{:.1} GB", f / 1e9) } else if f >= 1e6 { format!("{:.1} MB", f / 1e6) } else if f >= 1e3 { format!("{:.0} KB", f / 1e3) } else { format!("{n} 字节") }
}
/// 发文件、文件夹给一台连着的电脑
fn send_files(id: &str, paths: &[String]) {
    slog(&format!("要发 {} 项给 {id}", paths.len()));
    if paths.is_empty() { return }
    let (cid, list) = (cs(id), cs(&paths.join("\n")));
    let mut x = 0;
    with_core(|h| x = ywk_send_files(h, cid.as_ptr(), list.as_ptr()));
    let first = std::path::Path::new(&paths[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    app(|a| {
        let who = a.machines.get(id).map_or("另一台电脑".to_string(), |m| m.name.clone());
        let what = format!("{first}{}", if paths.len() > 1 { format!(" 等 {} 个", paths.len()) } else { String::new() });
        let mut j = Job { key: format!("s{x}"), x, peer: id.to_string(), sending: true, title: format!("发给{who}：{what}"), done: 0, total: 0, state: 0, err: String::new(), ended: None };
        if x == 0 { j.state = 2; j.err = "没连上".into(); j.ended = Some(std::time::Instant::now()); j.key = format!("s0-{}", a.jobs.len()) }
        a.jobs.push(j);
    });
    unsafe { SetTimer(MAIN.load(SeqCst), TIMER_JOBS, 500, std::ptr::null()); }
    panel::refresh();
}
fn cancel_job(key: &str) {
    let Some(j) = app(|a| a.jobs.iter().find(|j| j.key == key && j.state == 0).cloned()) else { return };
    let peer = cs(&j.peer);
    with_core(|h| if j.sending { ywk_cancel_send(h, j.x) } else { ywk_cancel_recv(h, peer.as_ptr(), j.x) });
}
fn set_recv_dir(d: String) {
    app(|a| { a.store.recv_dir = d.clone(); a.store.save() });
    let c = cs(&d);
    with_core(|h| ywk_set_recv_dir(h, c.as_ptr()));
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_NOTICE => { let s = Box::from_raw(lp as *mut String); handle_notice(&s); 0 }
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP | WM_RBUTTONUP => panel::toggle(),
                // 点了“收到文件”的通知：在资源管理器里打开，选中收到的东西
                NIN_BALLOONUSERCLICK => { let p = app(|a| if a.balloon_recv { a.last_recv.clone() } else { String::new() }); if !p.is_empty() { ShellExecuteW(0, w("open").as_ptr(), w("explorer.exe").as_ptr(), w(&format!("/select,\"{p}\"")).as_ptr(), std::ptr::null(), 1); } }
                _ => {}
            }
            0
        }
        // 完成、失败的几行过几秒拿掉
        WM_TIMER if wp == TIMER_JOBS => {
            let (changed, empty) = app(|a| { let n = a.jobs.len(); a.jobs.retain(|j| j.ended.map_or(true, |e| e.elapsed().as_secs() < if j.state == 1 { 3 } else { 6 })); (n != a.jobs.len(), a.jobs.is_empty()) });
            if empty { KillTimer(h, TIMER_JOBS); }
            panel::refresh();
            let _ = changed;
            0
        }
        // 对方每 2 秒广播一次，核心半分钟才转告一次：70 秒没消息就当它不在了
        WM_TIMER if wp == TIMER_SWEEP => {
            check_update();
            let gone = app(|a| { let n = a.found.len(); a.found.retain(|f| f.joining || f.seen.elapsed().as_secs() < 70); n != a.found.len() });
            if gone { panel::refresh() }
            0
        }
        WM_GRABBED => {
            let id = Box::from_raw(lp as *mut String);
            slog("鼠标穿走：本机指针停到中间、藏起来");
            app(|a| if a.problem.as_deref().map_or(false, |p| p.contains("没有响应")) { a.problem = None });
            if REMOTE.load(SeqCst) { hide_cursor(); flow(|f, b| f.grabbed(b, &id)); SetTimer(h, TIMER_WATCH, 500, std::ptr::null()); }
            0
        }
        // 看门狗：鼠标在别的电脑上时，本机指针应该一直停在停靠点。被挪走了说明钩子被系统摘了，马上拉回本机、重装钩子
        WM_TIMER if wp == TIMER_WATCH => {
            if !REMOTE.load(SeqCst) { KillTimer(h, TIMER_WATCH); return 0 }
            let mut cur = POINT::default(); GetCursorPos(&mut cur);
            let park = (PARK.0.load(SeqCst), PARK.1.load(SeqCst));
            if (cur.x - park.0).abs() > 200 || (cur.y - park.1).abs() > 200 { slog("看门狗：指针离开停靠点，钩子可能失效，拉回本机并重装"); go_home(); stop_hooks(); start_hooks(); }
            0
        }
        // 锁屏、切换用户：钩子收不到后面的按键，先把鼠标拉回本机（对方会松开所有键）
        // 锁屏期间松开的修饰键监听不到：告诉核心现在一个都没按着
        WM_WTSSESSION_CHANGE => { slog(&format!("会话变化 {wp}（锁屏等），拉回本机")); go_home(); with_core(|c| ywk_sync_mods(c, 0)); 0 }
        WM_RELEASED => { if !REMOTE.load(SeqCst) { show_cursor(); flow(|f, _| f.released()) } panel::refresh(); 0 }
        WM_PEER_CAME => { flow(|f, b| f.guest_entered(b)); 0 }
        WM_PEER_LEFT => {
            // 对方的指针走了：它在这儿复制的带回给它，本机的剪贴板恢复原样
            flow(|f, b| f.guest_left(b, wp as i32));
            // 正常走的：本机指针先藏起来放中间，本机鼠标一动再出现（只看得到一个指针）。
            // 这条消息排队的时候对方可能又进来了：那就不藏，不然它控制期间本机指针一直看不见
            if wp == 0 && !REMOTE.load(SeqCst) && !BOSSED.load(SeqCst) { SetCursorPos(PARK.0.load(SeqCst), PARK.1.load(SeqCst)); hide_cursor(); IDLE_HIDDEN.store(true, SeqCst); }
            0
        }
        WM_SHOW_LOCAL => { if !REMOTE.load(SeqCst) { show_cursor() } 0 }
        WM_SET_CAPS => {
            if (GetKeyState(0x14) & 1 != 0) != (wp != 0) {
                let k = |up: u32| INPUT { kind: INPUT_KEYBOARD, u: INPUT_U { ki: KEYBDINPUT { wVk: 0x14, wScan: 0x3A, dwFlags: up, time: 0, dwExtraInfo: MARK } } };
                send(&[k(0), k(KEYEVENTF_KEYUP)]);
            }
            0
        }
        WM_TIMER if wp == TIMER_PLACE => { KillTimer(h, TIMER_PLACE); for id in app(|a| std::mem::take(&mut a.pending_place)) { place_if_needed(&id) } 0 }
        WM_DISPLAYCHANGE => {
            read_screen();
            if REMOTE.load(SeqCst) { SetCursorPos(PARK.0.load(SeqCst), PARK.1.load(SeqCst)); }
            with_core(|c| ywk_set_size(c, SIZE.0.load(SeqCst) as u32, SIZE.1.load(SeqCst) as u32));
            DefWindowProcW(h, m, wp, lp)
        }
        WM_DESTROY => { PostQuitMessage(0); 0 }
        _ if m != 0 && m == TASKBAR_CREATED.load(SeqCst) => { tray(NIM_ADD, None); 0 }   // 资源管理器重启后托盘图标要重新加
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

fn main() {
    unsafe {
        // 按真实像素算坐标：由 exe 里的清单声明（PerMonitorV2），老系统上也不会因为找不到函数而打不开
        std::panic::set_hook(Box::new(|_| show_cursor()));       // 万一出错退出，先把指针恢复
        // 只开一个。旧版换新版时，旧的那个正在退出：等它几秒
        let mut tries = 0;
        loop {
            let m = CreateMutexW(std::ptr::null(), 1, w("Local\\YiweiKeyMouseShare").as_ptr());
            if GetLastError() != ERROR_ALREADY_EXISTS { break }
            CloseHandle(m);
            tries += 1;
            if tries > 20 {
                MessageBoxW(0, w("一苇已在运行，图标在任务栏右下角").as_ptr(), w("一苇").as_ptr(), MB_OK | MB_SETFOREGROUND);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        UPDATE.with(|u| *u.borrow_mut() = (exe_stamp(), None));
        show_cursor();                                           // 上次万一没放回来，先恢复系统指针
        let inst = GetModuleHandleW(std::ptr::null());
        let class = w("YiweiWindow");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: 0, lpfnWndProc: Some(wndproc), cbClsExtra: 0, cbWndExtra: 0,
                               hInstance: inst, hIcon: 0, hCursor: 0, hbrBackground: 0, lpszMenuName: std::ptr::null(), lpszClassName: class.as_ptr(), hIconSm: 0 };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), w("一苇").as_ptr(), 0, 0, 0, 0, 0, 0, 0, inst, std::ptr::null());
        MAIN.store(hwnd, SeqCst);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w("TaskbarCreated").as_ptr()), SeqCst);
        WTSRegisterSessionNotification(hwnd, 0);
        let png = include_bytes!("../res/icon32.png");
        let icon = CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, 32, 32, 0);
        let store = Store::load();
        let enabled = store.enabled;
        APP.with(|a| *a.borrow_mut() = Some(App { code: String::new(), jobs: Vec::new(), last_recv: String::new(), balloon_recv: false, store, machines: HashMap::new(), found: Vec::new(), told: HashSet::new(), pending_place: Vec::new(), icon, problem: None }));
        app(|a| for p in &a.store.peers { a.machines.insert(p.id.clone(), Machine { name: p.name.clone(), os: p.os, online: false }); });
        if enabled { start_share() }
        SetTimer(hwnd, TIMER_SWEEP, 10_000, std::ptr::null());
        tray(NIM_ADD, Some(("一苇已启动", "点任务栏右下角的一苇图标打开面板")));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            if msg.message == WM_KEYDOWN && panel::on_edit_key(msg.hwnd, msg.wParam) { continue }   // 面板里“手动添加”的输入框：回车、Esc
            TranslateMessage(&msg); DispatchMessageW(&msg);
        }
        stop_share();
        tray(NIM_DELETE, None);
    }
}
