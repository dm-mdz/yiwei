//! 给各系统外壳用的 C 接口。说明见 `core/include/yiwei_core.h`。
//!
//! 线程：
//!   · 外壳的键鼠监听回调调 ywk_local_move、ywk_key 这些，只经过 `Front`：读几个原子量、把事件放进队列就返回，
//!     不加密、不发网络、不等锁。真正的判断由后台线程 `work` 取出来做。
//!   · 别的电脑发来的输入，由引擎的收包线程交给控制器。
//!   · 这两处都拿着控制器的锁把结果交给外壳（`Core::run`），所以外壳看到的“穿走、回来、对方进来、对方走了”
//!     的先后和实际发生的先后一致。外壳在这几个回调里不能再调会用到控制器的 ywk_ 函数（头文件里写了是哪些）。

use crate::control::{Cmd, Controller, Link};
use crate::front::{Ev, Front};
use crate::input::{Action, Event};
use crate::layout::Layout;
use crate::net::{Config, Engine, Notice, Wire, DEFAULT_PORT};
use crate::secure::{Group, Identity};
use crate::wire::{ClipTag, DeviceId, Os, PROTOCOL};
use std::ffi::{c_char, c_void, CStr, CString};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

type NoticeFn = Option<extern "C" fn(*mut c_void, *const c_char)>;
type InputFn = Option<extern "C" fn(*mut c_void, i32, i32, i32, i32, i32)>;

/// 调外壳的回调。alive 在 ywk_stop 里拿写锁关掉：stop 返回之后再也不会调回调
#[derive(Clone)]
struct Out { notice: NoticeFn, input: InputFn, ctx: *mut c_void, alive: Arc<RwLock<bool>> }
unsafe impl Send for Out {}
unsafe impl Sync for Out {}
impl Out {
    fn say(&self, json: String) {
        // 剪贴板的内容不进运行记录（可能是密码），只记多长
        // 组的校验码和组钥匙更不能记：拿到运行记录的人凭组钥匙就成了组员
        if json.starts_with(r#"{"t":"clip"#) { crate::ylog!("通知 clip（{} 字节，内容不记：文字、图片和文件的名字都算）", json.len()) }
        else if json.starts_with(r#"{"t":"group""#) { crate::ylog!("通知 group（{}，校验码和组钥匙不记）", if json.contains(r#""joined":1"#) { "加入了别的组" } else { "本组的校验码是新的" }) }
        else if !json.contains("_progress") { crate::ylog!("通知 {}", if json.len() > 300 { &json[..json.char_indices().nth(300).map_or(json.len(), |c| c.0)] } else { &json }) }
        let alive = self.alive.read().unwrap();
        if let (true, Some(f), Ok(c)) = (*alive, self.notice, CString::new(json)) { f(self.ctx, c.as_ptr()) }
    }
    fn input(&self, kind: i32, a: i32, b: i32, c: i32, d: i32) {
        if kind >= 5 { crate::ylog!("对方的输入 kind={kind} a={a} b={b} c={c}") }
        let alive = self.alive.read().unwrap();
        if let (true, Some(f)) = (*alive, self.input) { f(self.ctx, kind, a, b, c, d) }
    }
}

/// 被别的电脑控制期间的耗时统计，它走的时候写一行进运行记录：真机上“卡不卡、卡在哪一段”靠这个判断
#[derive(Default)]
struct Inbound { began: Option<Instant>, last: Option<Instant>, moves: u32, gaps: u32, worst_us: u64, slow: u32 }
impl Inbound {
    fn note(&mut self, kind: i32, a: i32, b: i32, at: Instant, took_us: u64) {
        if kind == 5 { *self = Inbound { began: Some(at), ..Default::default() } }
        if kind == 1 { crate::log::trace(3, [a as f64, b as f64, took_us as f64 / 1000.0, 0.0, 0.0, 0.0]) }
        self.worst_us = self.worst_us.max(took_us);
        if took_us > 2000 { self.slow += 1 }
        if kind == 1 {
            // 鼠标在动的时候两次位置隔了 25–120 毫秒：多半是卡了一下（更长的当作手停了）
            if let Some(l) = self.last { if (25..=120).contains(&at.duration_since(l).as_millis()) { self.gaps += 1 } }
            self.last = Some(at); self.moves += 1;
        }
        if kind == 6 {
            if let Some(b) = self.began.take() {
                crate::ylog!("这次被别的电脑控制 {:.1} 秒：收到鼠标位置 {} 次，其中和上一次隔了 25–120 毫秒的 {} 次；照做一次输入最长 {:.1} 毫秒（超过 2 毫秒的 {} 次）",
                    at.duration_since(b).as_secs_f64(), self.moves, self.gaps, self.worst_us as f64 / 1000.0, self.slow);
                crate::log::trace_dump("被控");
            }
        }
    }
}

/// 控制器、监听回调用的那一层、回调外壳的口子：后台线程、引擎的回调、C 接口共用
struct Core { control: Mutex<Controller>, front: Front, out: Out, wire: OnceLock<Wire>, started: Instant, inbound: Mutex<Inbound> }
impl Core {
    /// 拿着控制器的锁做一件事，并把它要外壳做的事按顺序交给外壳。锁着交：两个线程各自的结果，交出去的先后和发生的先后一致
    fn run(&self, f: impl FnOnce(&mut Controller, &dyn Link, &mut Vec<Cmd>)) {
        let Some(wire) = self.wire.get() else { return };
        let mut c = self.control.lock().unwrap();
        let mut cmds = Vec::new();
        f(&mut c, wire, &mut cmds);
        self.front.set_bossed(c.hosting());
        let back = cmds.iter().any(|c| matches!(c, Cmd::Release { .. }));
        for cmd in cmds {
            match cmd {
                Cmd::Grab { peer } => { self.front.set_remote(true); self.out.say(format!(r#"{{"t":"grab","id":"{}"}}"#, hex(&peer))) }
                Cmd::Release { x, y } => { self.front.set_remote(false); self.out.say(format!(r#"{{"t":"release","x":{x},"y":{y}}}"#)) }
                Cmd::Input { kind, a, b, c, d } => {
                    let t = Instant::now();
                    self.out.input(kind, a, b, c, d);
                    self.inbound.lock().unwrap().note(kind, a, b, t, t.elapsed().as_micros() as u64);
                }
            }
        }
        if back { self.front.wake() }                                           // 鼠标回来了：让后台线程把这次控制的统计写下来
    }
}

/// 控制别的电脑期间的耗时统计，鼠标回来时写一行进运行记录（和被控那边的对着看，就知道卡在哪一段）
struct Outbound { on: bool, began: Instant, last_move: Option<Instant>, batches: u32, late: u32, gaps: u32, worst_wait: u64, worst_work: u64 }
impl Outbound {
    fn finish(&mut self) {
        self.on = false;
        crate::ylog!("这次控制别的电脑 {:.1} 秒：处理输入 {} 批，其中鼠标移动和上一次隔了 25–120 毫秒的 {} 次；从监听回调到开始处理最长 {:.1} 毫秒（超过 3 毫秒的 {} 批）；处理一批最长 {:.1} 毫秒",
            self.began.elapsed().as_secs_f64(), self.batches, self.gaps, self.worst_wait as f64 / 1000.0, self.late, self.worst_work as f64 / 1000.0);
        crate::log::trace_dump("控制");
    }
}

/// 后台线程：把监听回调放进队列的事件取出来，交给控制器判断、发给别的电脑。没事的时候一直睡着
fn work(core: Arc<Core>) {
    crate::urgent_thread();
    let mut batch: Vec<Ev> = Vec::with_capacity(64);
    let mut st = Outbound { on: false, began: Instant::now(), last_move: None, batches: 0, late: 0, gaps: 0, worst_wait: 0, worst_work: 0 };
    while !core.front.stopping() {
        let home = core.front.take_home();
        let swallowed = core.front.take_swallowed();
        if swallowed > 0 { crate::ylog!("修饰键的按下有 {swallowed} 次没发过去：本机记着它从穿走之前就一直按着（抬起那一下没监听到）") }
        core.front.drain(&mut batch);
        let waited = core.front.take_wait_us();
        if !home && batch.is_empty() {
            if st.on && !core.front.remote() { st.finish() }                    // 鼠标是被对方请回来的（不是这个线程处理的）
            std::thread::park();
            continue;
        }
        let lost = core.front.take_dropped();
        if lost > 0 { crate::ylog!("后台线程没来得及处理，丢了 {lost} 次输入") }
        let mut moved = false;
        for e in &batch { if let Ev::RemoteMove { dx, dy } = e { moved = true; crate::log::trace(2, [*dx, *dy, waited as f64 / 1000.0, 0.0, 0.0, 0.0]) } }
        let t = Instant::now();
        core.run(|c, link, out| {
            c.set_caps(core.front.caps());
            c.set_now(core.started.elapsed().as_millis() as u64);
            if home { crate::ylog!("紧急拉回"); out.extend(c.go_home(link)) }
            for ev in batch.drain(..) {
                match ev {
                    Ev::LocalMove { x, y, dx, dy } => out.extend(c.local_move(link, x, y, dx, dy)),
                    Ev::RemoteMove { dx, dy } => c.remote_move(link, dx, dy),
                    Ev::Key { usage, down, taken } => out.extend(c.key(link, usage, down, taken)),
                    Ev::Button { button, down, taken } => out.extend(c.button(link, button, down, taken)),
                    Ev::Wheel { dx, dy, taken } => c.wheel(link, dx, dy, taken),
                }
            }
        });
        batch.clear();
        let work = t.elapsed().as_micros() as u64;
        let remote = core.front.remote();
        if remote && !st.on { st = Outbound { on: true, began: t, last_move: None, batches: 0, late: 0, gaps: 0, worst_wait: 0, worst_work: 0 } }
        if st.on {
            st.batches += 1;
            if waited > 3000 { st.late += 1 }
            st.worst_wait = st.worst_wait.max(waited); st.worst_work = st.worst_work.max(work);
            if moved {
                if let Some(l) = st.last_move { if (25..=120).contains(&t.duration_since(l).as_millis()) { st.gaps += 1 } }
                st.last_move = Some(t);
            }
            if !remote { st.finish() }
        }
    }
}

pub struct Handle { engine: Engine, core: Arc<Core>, worker: Mutex<Option<std::thread::JoinHandle<()>>> }

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 { return None }
    let mut out = [0u8; N];
    for i in 0..N { out[i] = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()? }
    Some(out)
}
fn text(p: *const c_char) -> Option<String> { if p.is_null() { None } else { unsafe { CStr::from_ptr(p) }.to_str().ok().map(|s| s.to_string()) } }
/// 把字符串写成 JSON 里带引号的样子
fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""), '\\' => o.push_str("\\\\"), '\n' => o.push_str("\\n"), '\r' => o.push_str("\\r"), '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
fn layout_json(rev: u64, cells: &[(DeviceId, i16, i16)]) -> String {
    let list: Vec<String> = cells.iter().map(|(id, x, y)| format!(r#"["{}",{x},{y}]"#, hex(id))).collect();
    format!(r#"{{"t":"layout","rev":{rev},"cells":[{}]}}"#, list.join(","))
}

#[no_mangle]
pub extern "C" fn ywk_new_identity() -> *mut c_char {
    match Identity::generate() {
        Ok(id) => CString::new(format!("{} {}", hex(&id.private), hex(&id.public))).map_or(std::ptr::null_mut(), |c| c.into_raw()),
        Err(_) => std::ptr::null_mut(),
    }
}
#[no_mangle]
pub extern "C" fn ywk_free(s: *mut c_char) { if !s.is_null() { unsafe { drop(CString::from_raw(s)) } } }

#[no_mangle]
pub extern "C" fn ywk_start(id: *const c_char, name: *const c_char, os: i32, width: u32, height: u32,
                            private_key: *const c_char, public_key: *const c_char, group: *const c_char,
                            port: u16, discover: i32, on_notice: NoticeFn, on_input: InputFn, ctx: *mut c_void) -> *mut Handle {
    let made = (|| -> Option<Handle> {
        let me: DeviceId = unhex(&text(id)?)?;
        let os = match os { 1 => Os::Mac, 2 => Os::Windows, 3 => Os::Linux, _ => return None };
        let identity = Identity { private: unhex(&text(private_key)?)?, public: unhex(&text(public_key)?)? };
        // 这台电脑所在的组：外壳存着的那一行。没有（第一次用、旧版升上来）就自己成一组，并让外壳存下来
        let saved = text(group).and_then(|t| Group::from_text(&t));
        let fresh = saved.is_none();
        let group = match saved { Some(g) => g, None => Group::generate(1).ok()? };
        let cfg = Config { id: me, name: text(name)?, os, width, height, identity, group: group.clone(),
                           port: if port == 0 { DEFAULT_PORT } else { port }, discover: discover != 0 };
        let core = Arc::new(Core {
            control: Mutex::new(Controller::new(me, width, height)), front: Front::new(width, height),
            out: Out { notice: on_notice, input: on_input, ctx, alive: Arc::new(RwLock::new(true)) }, wire: OnceLock::new(), started: Instant::now(), inbound: Mutex::new(Inbound::default()),
        });
        let k = core.clone();
        let engine = Engine::start(cfg, move |n| { let out = &k.out; match n {
            Notice::Found { id, name, os, addr } => out.say(format!(r#"{{"t":"found","id":"{}","name":{},"os":{},"ip":"{}","port":{}}}"#, hex(&id), quote(&name), os as u8, addr.ip(), addr.port())),
            Notice::Group { code, save, joined } => {
                // 加入了别人的组：原来的摆法不要了
                if joined { k.control.lock().unwrap().set_layout(Layout::default()); }
                out.say(group_json(&code, &save, joined));
            }
            Notice::JoinRequest { id, name, os } => out.say(format!(r#"{{"t":"ask","id":"{}","name":{},"os":{}}}"#, hex(&id), quote(&name), os as u8)),
            Notice::JoinFailed { name, why } => out.say(format!(r#"{{"t":"join_failed","name":{},"why":{why}}}"#, quote(&name))),
            Notice::Stalled { name } => out.say(format!(r#"{{"t":"stalled","name":{}}}"#, quote(&name))),
            Notice::Mismatch { name, theirs } => out.say(format!(r#"{{"t":"mismatch","name":{},"theirs":{theirs},"mine":{PROTOCOL}}}"#, quote(&name))),
            Notice::Connected { id, name, os, width, height } => {
                k.control.lock().unwrap().peer_connected(id, width, height);
                out.say(format!(r#"{{"t":"connected","id":"{}","name":{},"os":{},"w":{width},"h":{height}}}"#, hex(&id), quote(&name), os as u8));
            }
            Notice::Disconnected { id } => {
                // 鼠标正在它上面：回本机；它的鼠标正在本机上：替它松开按着的键和鼠标键
                k.run(|c, _, cmds| cmds.extend(c.peer_disconnected(id)));
                out.say(format!(r#"{{"t":"disconnected","id":"{}"}}"#, hex(&id)));
            }
            Notice::SendProgress { xfer, id, done, total } => out.say(format!(r#"{{"t":"send_progress","x":{xfer},"id":"{}","done":{done},"total":{total}}}"#, hex(&id))),
            Notice::SendDone { xfer, id, ok, err } => out.say(format!(r#"{{"t":"send_done","x":{xfer},"id":"{}","ok":{ok},"err":{}}}"#, hex(&id), quote(&err))),
            Notice::RecvBegin { xfer, id, name, count, total } => out.say(format!(r#"{{"t":"recv_begin","x":{xfer},"id":"{}","name":{},"count":{count},"total":{total}}}"#, hex(&id), quote(&name))),
            Notice::RecvProgress { xfer, id, done, total } => out.say(format!(r#"{{"t":"recv_progress","x":{xfer},"id":"{}","done":{done},"total":{total}}}"#, hex(&id))),
            Notice::RecvDone { xfer, id, ok, err, first, count, total } => out.say(format!(r#"{{"t":"recv_done","x":{xfer},"id":"{}","ok":{ok},"err":{},"first":{},"count":{count},"total":{total}}}"#,
                hex(&id), quote(&err), quote(&first.to_string_lossy()))),
            Notice::Resized { id, width, height } => k.control.lock().unwrap().peer_connected(id, width, height),
            Notice::ClipText { id, text, tag } => out.say(format!(r#"{{"t":"clip","id":"{}","back":{},"no":{},"text":{}}}"#, hex(&id), tag.back as u8, tag.no, quote(&text))),
            Notice::ClipImage { id, path, tag } => out.say(format!(r#"{{"t":"clip_image","id":"{}","back":{},"no":{},"path":{}}}"#, hex(&id), tag.back as u8, tag.no, quote(&path.to_string_lossy()))),
            Notice::ClipFiles { id, paths, tag } => out.say(format!(r#"{{"t":"clip_files","id":"{}","back":{},"no":{},"paths":[{}]}}"#, hex(&id), tag.back as u8, tag.no,
                paths.iter().map(|p| quote(&p.to_string_lossy())).collect::<Vec<_>>().join(","))),
            Notice::ClipFailed { id } => out.say(format!(r#"{{"t":"clip_failed","id":"{}"}}"#, hex(&id))),
            Notice::Layout { rev, cells } => {
                let mut l = Layout::default(); l.merge(rev, &cells);
                k.control.lock().unwrap().set_layout(l);
                out.say(layout_json(rev, &cells));
            }
            Notice::Input { id, actions } => {
                // 有电脑的鼠标要进来了：先告诉外壳是哪一台（剪贴板跟着指针走，外壳要知道这位客人是谁）
                if actions.iter().any(|a| matches!(a, Action::Event(Event::Enter { .. }))) { out.say(format!(r#"{{"t":"guest","id":"{}"}}"#, hex(&id))) }
                k.run(|c, link, cmds| {
                    let now = k.started.elapsed().as_millis() as u64;
                    for a in actions { cmds.extend(c.incoming(link, id, a, now)) }
                })
            }
        } }).ok()?;
        let _ = core.wire.set(engine.wire());
        let k = core.clone();
        let worker = std::thread::Builder::new().name("yiwei-input".into()).spawn(move || {
            // 这个线程要是出错停了，键鼠就没人管了（鼠标正在别的电脑上的话用户会被困住）：宁可整个程序退出，系统会把键鼠还给本机
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(k))).is_err() {
                crate::ylog!("后台线程出错了，程序退出，把键鼠还给本机");
                std::process::abort();
            }
        }).ok()?;
        core.front.set_worker(worker.thread().clone());
        crate::ylog!("核心启动：协议第 {PROTOCOL} 版，端口 {}", engine.port());
        if fresh { core.out.say(group_json(&group.code, &group.to_text(), false)) }
        Some(Handle { engine, core, worker: Mutex::new(Some(worker)) })
    })();
    made.map_or(std::ptr::null_mut(), |h| Box::into_raw(Box::new(h)))
}

fn group_json(code: &str, save: &str, joined: bool) -> String { format!(r#"{{"t":"group","code":"{code}","save":"{save}","joined":{}}}"#, joined as u8) }
fn with<R>(h: *mut Handle, default: R, f: impl FnOnce(&Handle) -> R) -> R { if h.is_null() { default } else { f(unsafe { &*h }) } }

#[no_mangle]
pub extern "C" fn ywk_stop(h: *mut Handle) {
    if h.is_null() { return }
    let h = unsafe { Box::from_raw(h) };
    h.core.front.request_stop();
    if let Some(w) = h.worker.lock().unwrap().take() { let _ = w.join(); }
    // 鼠标还在别的电脑上（包括后台线程最后一刻才穿过去的）：拉回来。外壳会收到 release，对方收到“走了”，不用等掉线才松键
    h.core.run(|c, link, out| out.extend(c.go_home(link)));
    *h.core.out.alive.write().unwrap() = false;            // 等正在调的回调返回，之后不再调
    h.engine.stop();
}
#[no_mangle]
pub extern "C" fn ywk_port(h: *mut Handle) -> u16 { with(h, 0, |h| h.engine.port()) }
#[no_mangle]
pub extern "C" fn ywk_join(h: *mut Handle, ip: *const c_char, port: u16, code: *const c_char) {
    with(h, (), |h| { if let Some(ip) = text(ip).and_then(|s| s.parse::<IpAddr>().ok()) { h.engine.join(SocketAddr::new(ip, port), &text(code).unwrap_or_default()) } })
}
/// 本机这一组现在的校验码。用完调 ywk_free
#[no_mangle]
pub extern "C" fn ywk_code(h: *mut Handle) -> *mut c_char {
    with(h, std::ptr::null_mut(), |h| CString::new(h.engine.group().code).map_or(std::ptr::null_mut(), |s| s.into_raw()))
}
#[no_mangle]
pub extern "C" fn ywk_new_code(h: *mut Handle) { with(h, (), |h| h.engine.new_code()) }
#[no_mangle]
pub extern "C" fn ywk_confirm(h: *mut Handle, id: *const c_char, ok: i32) {
    with(h, (), |h| { if let Some(id) = text(id).and_then(|s| unhex::<16>(&s)) { h.engine.confirm_join(id, ok != 0) } })
}
#[no_mangle]
pub extern "C" fn ywk_place(h: *mut Handle, id: *const c_char, x: i32, y: i32) {
    with(h, (), |h| {
        let Some(id) = text(id).and_then(|s| unhex::<16>(&s)) else { return };
        h.engine.place(id, x as i16, y as i16);
        let l = h.engine.layout();
        h.core.out.say(layout_json(l.rev, l.cells()));
        h.core.control.lock().unwrap().set_layout(l);
    })
}
#[no_mangle]
pub extern "C" fn ywk_forget(h: *mut Handle, id: *const c_char) {
    with(h, (), |h| {
        let Some(id) = text(id).and_then(|s| unhex::<16>(&s)) else { return };
        h.engine.forget(id);
        let l = h.engine.layout();
        h.core.out.say(layout_json(l.rev, l.cells()));
        h.core.control.lock().unwrap().set_layout(l);
    })
}
#[no_mangle]
pub extern "C" fn ywk_set_size(h: *mut Handle, width: u32, height: u32) {
    with(h, (), |h| { h.core.control.lock().unwrap().set_size(width, height); h.core.front.set_size(width, height); h.engine.set_size(width, height) })
}
/// 启动后把上次存下的摆法放回来。cells：“编号:x:y”用逗号隔开
#[no_mangle]
pub extern "C" fn ywk_restore_layout(h: *mut Handle, rev: u64, cells: *const c_char) {
    with(h, (), |h| {
        let mut list = Vec::new();
        for item in text(cells).unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
            let mut it = item.split(':');
            let (Some(id), Some(x), Some(y)) = (it.next().and_then(unhex::<16>), it.next().and_then(|v| v.parse().ok()), it.next().and_then(|v| v.parse().ok())) else { return };
            list.push((id, x, y));
        }
        h.engine.restore_layout(rev, &list);
        h.core.control.lock().unwrap().set_layout(h.engine.layout());
    })
}
/// 把剪贴板里的文字带给 id 这台电脑。back = 0：本机的指针进到它那儿，带过去（空的 = 这次没有能带的）；
/// back = 1：它的指针在本机上时复制的，它的指针走了，带回给它。no：本机第几次往外带（同一次带的文字和图片用同一个）
#[no_mangle]
pub extern "C" fn ywk_clip(h: *mut Handle, id: *const c_char, s: *const c_char, back: i32, no: u32) {
    with(h, (), |h| {
        if let (Some(peer), Some(t)) = (text(id).and_then(|s| unhex::<16>(&s)), text(s)) { h.engine.send_clip_text(peer, &t, ClipTag { back: back != 0, no }); }
    })
}
/// 同上，剪贴板里的图片（外壳已经存成 path 这个文件）；发完这个文件由核心删掉
#[no_mangle]
pub extern "C" fn ywk_clip_image(h: *mut Handle, id: *const c_char, path: *const c_char, back: i32, no: u32) {
    with(h, (), |h| {
        let Some(p) = text(path) else { return };
        match text(id).and_then(|s| unhex::<16>(&s)) { Some(peer) => { h.engine.send_clip_image(peer, p.into(), ClipTag { back: back != 0, no }); } None => { let _ = std::fs::remove_file(p); } }
    })
}
/// 同上，复制的文件、文件夹（paths：本机路径，一行一个；原件不动）。合起来 100MB 以内才带。
/// 返回 1 带了、0 太大（或者多于 2000 个）没带、-1 没连着或者里面没有文件
#[no_mangle]
pub extern "C" fn ywk_clip_files(h: *mut Handle, id: *const c_char, paths: *const c_char, back: i32, no: u32) -> i32 {
    with(h, -1, |h| {
        let (Some(id), Some(p)) = (text(id).and_then(|s| unhex::<16>(&s)), text(paths)) else { return -1 };
        let list: Vec<std::path::PathBuf> = p.lines().filter(|l| !l.is_empty()).map(std::path::PathBuf::from).collect();
        if list.is_empty() { return -1 }
        let r = h.engine.send_clip_files(id, list.clone(), ClipTag { back: back != 0, no });
        crate::ylog!("复制的 {} 个文件（文件夹）带给 {}：{}", list.len(), hex(&id)[..8].to_string(), match r { Some(true) => "带了", Some(false) => "太大或者太多，没带", None => "没连着，或者里面没有文件" });
        match r { Some(true) => 1, Some(false) => 0, None => -1 }
    })
}
/// 给一台电脑发文件、文件夹。paths：本机路径，一行一个。返回这次的编号（0 表示没连上）
#[no_mangle]
pub extern "C" fn ywk_send_files(h: *mut Handle, id: *const c_char, paths: *const c_char) -> u32 {
    with(h, 0, |h| {
        let (Some(id), Some(p)) = (text(id).and_then(|s| unhex::<16>(&s)), text(paths)) else { return 0 };
        let list: Vec<std::path::PathBuf> = p.lines().filter(|l| !l.is_empty()).map(std::path::PathBuf::from).collect();
        if list.is_empty() { return 0 }
        let x = h.engine.send_files(id, list.clone()).unwrap_or(0);
        crate::ylog!("发文件给 {} 编号 {x}：{:?}", hex(&id)[..8].to_string(), list);
        x
    })
}
#[no_mangle]
pub extern "C" fn ywk_cancel_send(h: *mut Handle, xfer: u32) { with(h, (), |h| h.engine.cancel_send(xfer)) }
#[no_mangle]
pub extern "C" fn ywk_cancel_recv(h: *mut Handle, id: *const c_char, xfer: u32) {
    with(h, (), |h| if let Some(id) = text(id).and_then(|s| unhex::<16>(&s)) { h.engine.cancel_recv(id, xfer) })
}
#[no_mangle]
pub extern "C" fn ywk_set_recv_dir(h: *mut Handle, dir: *const c_char) {
    with(h, (), |h| if let Some(d) = text(dir).filter(|d| !d.is_empty()) { h.engine.set_recv_dir(d.into()) })
}
/// 打开运行记录（写到这个文件）
#[no_mangle]
pub extern "C" fn ywk_set_log(path: *const c_char) { if let Some(p) = text(path) { crate::log::open(std::path::Path::new(&p)) } }
/// 外壳记一行
#[no_mangle]
pub extern "C" fn ywk_log(s: *const c_char) { if let Some(t) = text(s) { crate::ylog!("外壳 {t}") } }

// —— 下面这几个是监听回调里调的：只碰 Front，马上返回 ——

/// “鼠标被控切回”：开着（on = 1）时，别的电脑的指针在本机上，本机自己的鼠标一动就请它回去（它的指针回到它自己屏幕中间）。默认关
#[no_mangle]
pub extern "C" fn ywk_set_take_back(h: *mut Handle, on: i32) { with(h, (), |h| h.core.control.lock().unwrap().set_take_back(on != 0)) }
#[no_mangle]
pub extern "C" fn ywk_home(h: *mut Handle) { with(h, (), |h| h.core.front.request_home()) }
#[no_mangle]
pub extern "C" fn ywk_local_move(h: *mut Handle, x: f64, y: f64, dx: f64, dy: f64) { with(h, (), |h| h.core.front.local_move(x, y, dx, dy)) }
#[no_mangle]
pub extern "C" fn ywk_remote_move(h: *mut Handle, dx: f64, dy: f64) { with(h, (), |h| h.core.front.remote_move(dx, dy)) }
#[no_mangle]
pub extern "C" fn ywk_key(h: *mut Handle, usage: i32, down: i32) -> i32 { with(h, 0, |h| h.core.front.key(usage as u16, down != 0) as i32) }
#[no_mangle]
pub extern "C" fn ywk_button(h: *mut Handle, button: i32, down: i32) -> i32 { with(h, 0, |h| h.core.front.button(button as u8, down != 0) as i32) }
#[no_mangle]
pub extern "C" fn ywk_wheel(h: *mut Handle, dx: i32, dy: i32) -> i32 { with(h, 0, |h| h.core.front.wheel(dx, dy) as i32) }
/// 排查用：外壳往轨迹里记一行（见 log.rs 里“轨迹”的说明）
#[no_mangle]
pub extern "C" fn ywk_trace(tag: i32, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) { crate::log::trace(tag as u8, [a, b, c, d, e, f]) }
#[no_mangle]
pub extern "C" fn ywk_sync_mods(h: *mut Handle, mask: u32) { with(h, (), |h| h.core.front.sync_mods(mask)) }
#[no_mangle]
pub extern "C" fn ywk_remote(h: *mut Handle) -> i32 { with(h, 0, |h| h.core.front.remote() as i32) }
#[no_mangle]
pub extern "C" fn ywk_usage_from_mac(keycode: i32) -> i32 { crate::keys::usage_from_mac(keycode as u16).map_or(-1, |u| u as i32) }
#[no_mangle]
pub extern "C" fn ywk_mac_from_usage(usage: i32) -> i32 { crate::keys::mac_from_usage(usage as u16).map_or(-1, |u| u as i32) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    static LOG_A: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static LOG_B: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static INPUT_A: Mutex<Vec<(i32, i32, i32, i32, i32)>> = Mutex::new(Vec::new());
    static INPUT_B: Mutex<Vec<(i32, i32, i32, i32, i32)>> = Mutex::new(Vec::new());
    extern "C" fn note_a(_: *mut c_void, j: *const c_char) { LOG_A.lock().unwrap().push(unsafe { CStr::from_ptr(j) }.to_str().unwrap().to_string()) }
    extern "C" fn note_b(_: *mut c_void, j: *const c_char) { LOG_B.lock().unwrap().push(unsafe { CStr::from_ptr(j) }.to_str().unwrap().to_string()) }
    extern "C" fn input_a(_: *mut c_void, k: i32, a: i32, b: i32, c: i32, d: i32) { INPUT_A.lock().unwrap().push((k, a, b, c, d)) }
    extern "C" fn input_b(_: *mut c_void, k: i32, a: i32, b: i32, c: i32, d: i32) { INPUT_B.lock().unwrap().push((k, a, b, c, d)) }

    fn c(s: &str) -> CString { CString::new(s).unwrap() }
    fn until(what: &str, mut f: impl FnMut() -> bool) { let end = Instant::now() + Duration::from_secs(5); while !f() { assert!(Instant::now() < end, "等了 5 秒：{what}"); std::thread::sleep(Duration::from_millis(5)) } }
    fn field(log: &Mutex<Vec<String>>, kind: &str, key: &str) -> Option<String> {
        let l = log.lock().unwrap();
        let line = l.iter().find(|j| j.contains(&format!(r#""t":"{kind}""#)))?;
        let at = line.find(&format!(r#""{key}":""#))? + key.len() + 4;
        Some(line[at..].split('"').next()?.to_string())
    }
    fn count(log: &Mutex<Vec<String>>, kind: &str) -> usize { log.lock().unwrap().iter().filter(|j| j.contains(&format!(r#""t":"{kind}""#))).count() }
    /// 甲把鼠标推出右边缘，等它穿到乙上
    fn cross(a: *mut Handle) { ywk_local_move(a, 1919.0, 540.0, 5.0, 0.0); until("甲的鼠标穿过去", || ywk_remote(a) == 1) }

    #[test]
    fn 外壳通过c接口走完配对和控制() {
        let ida = unsafe { CString::from_raw(ywk_new_identity()) }.into_string().unwrap();
        let idb = unsafe { CString::from_raw(ywk_new_identity()) }.into_string().unwrap();
        let (ka, kb) = (ida.split_once(' ').unwrap(), idb.split_once(' ').unwrap());
        assert_eq!((ka.0.len(), ka.1.len()), (64, 64));
        let (ia, ib) = ("01".repeat(16), "02".repeat(16));
        // 端口传一个不常用的固定值的话测试之间会抢，这里借“0 = 默认端口”之外的随机高位端口
        let pa = 40000 + (std::process::id() % 5000) as u16;
        let a = ywk_start(c(&ia).as_ptr(), c("书房的 \"Mac\"").as_ptr(), 1, 1920, 1080, c(ka.0).as_ptr(), c(ka.1).as_ptr(), c("").as_ptr(), pa, 0, Some(note_a), Some(input_a), std::ptr::null_mut());
        let b = ywk_start(c(&ib).as_ptr(), c("台式机").as_ptr(), 2, 2560, 1440, c(kb.0).as_ptr(), c(kb.1).as_ptr(), c("").as_ptr(), pa + 1, 0, Some(note_b), Some(input_b), std::ptr::null_mut());
        assert!(!a.is_null() && !b.is_null());
        assert!(ywk_start(c("xx").as_ptr(), c("坏的").as_ptr(), 1, 1, 1, c("").as_ptr(), c("").as_ptr(), c("").as_ptr(), 0, 0, Some(note_a), Some(input_a), std::ptr::null_mut()).is_null());

        // 两台各自成一组；第一次启动，让外壳把组存下来
        until("两边都报了自己的组", || field(&LOG_A, "group", "save").is_some() && field(&LOG_B, "group", "save").is_some());
        let code_b = unsafe { CString::from_raw(ywk_code(b)) }.into_string().unwrap();
        assert_eq!(field(&LOG_B, "group", "code").as_deref(), Some(code_b.as_str()));
        // 甲输入乙的校验码申请加入；乙这边问用户，用户点了允许
        ywk_join(a, c("127.0.0.1").as_ptr(), ywk_port(b), c(&code_b).as_ptr());
        until("乙被问到", || field(&LOG_B, "ask", "id").is_some());
        assert!(LOG_B.lock().unwrap().iter().any(|j| j.contains(r#""name":"书房的 \"Mac\"""#)), "名字里的引号要转义");
        ywk_confirm(b, c(&ia).as_ptr(), 1);
        until("两边都连上", || field(&LOG_A, "connected", "id").is_some() && field(&LOG_B, "connected", "id").is_some());
        // 甲改用乙那一组，外壳要存下新的
        let joined = LOG_A.lock().unwrap().iter().find(|j| j.contains(r#""t":"group""#) && j.contains(r#""joined":1"#)).cloned().expect("甲加入后要报新的组");
        assert!(joined.contains(&format!(r#""code":"{code_b}""#)));
        let saved_a = joined.split(r#""save":""#).nth(1).unwrap().split('"').next().unwrap().to_string();

        // 把台式机摆在 Mac 右边，然后把鼠标推出右边缘
        ywk_place(a, c(&ia).as_ptr(), 0, 0); ywk_place(a, c(&ib).as_ptr(), 1, 0);
        until("对方收到摆法", || LOG_B.lock().unwrap().iter().any(|j| j.contains(r#""t":"layout""#) && j.contains(&ib)));
        assert_eq!(ywk_remote(a), 0);
        assert_eq!(ywk_key(a, 0x04, 1), 0, "鼠标在本机时按键归本机");
        assert_eq!(ywk_key(a, 0x04, 0), 0);
        ywk_local_move(a, 900.0, 540.0, 5.0, 0.0);                           // 屏幕中间：什么都不发生
        cross(a);
        until("外壳被告知穿走了", || count(&LOG_A, "grab") == 1);
        until("乙被告知进来的是谁", || field(&LOG_B, "guest", "id").as_deref() == Some(ia.as_str()));
        assert_eq!(ywk_key(a, 0x04, 1), 1);
        assert_eq!(ywk_button(a, 1, 1), 1);
        assert_eq!(ywk_button(a, 1, 0), 1);
        assert_eq!(ywk_wheel(a, 0, -120), 1);
        ywk_remote_move(a, 100.0, 0.0);
        ywk_clip(a, c(&ib).as_ptr(), c("带过去的字").as_ptr(), 0, 1);
        until("对方收到输入", || { let i = INPUT_B.lock().unwrap();
            i.contains(&(5, 0, 500, 0, 0)) && i.contains(&(3, 4, 1, 0, 0)) && i.contains(&(1, 101, 720, 0, 0)) && i.contains(&(2, 0, -120, 0, 0)) && i.contains(&(4, 1, 0, 1, 720)) });
        until("对方收到剪贴板", || LOG_B.lock().unwrap().iter().any(|j| j.contains("带过去的字") && j.contains(r#""back":0,"no":1"#)));
        ywk_clip(b, c(&ia).as_ptr(), c("还回来的字").as_ptr(), 1, 2);
        until("剪贴板还回来", || LOG_A.lock().unwrap().iter().any(|j| j.contains("还回来的字") && j.contains(r#""back":1,"no":2"#)));
        ywk_remote_move(a, -500.0, 0.0);                                    // 走回来
        until("甲的鼠标回来", || ywk_remote(a) == 0);
        until("外壳被告知回来了", || LOG_A.lock().unwrap().iter().any(|j| j == r#"{"t":"release","x":1918,"y":540}"#));
        until("对方收到“走了”", || INPUT_B.lock().unwrap().contains(&(6, 0, 0, 0, 0)));

        // 发文件：甲发一个文件夹给乙
        let base = std::env::temp_dir().join(format!("yiwei-ffi-send-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("源/照片")).unwrap();
        std::fs::write(base.join("源/照片/大.bin"), vec![9u8; 3_000_000]).unwrap();
        std::fs::write(base.join("源/照片/小.txt"), "你好").unwrap();
        ywk_set_recv_dir(b, c(&base.join("收").to_string_lossy()).as_ptr());
        let x = ywk_send_files(a, c(&ib).as_ptr(), c(&format!("{}\n", base.join("源/照片").display())).as_ptr());
        assert!(x > 0);
        until("乙收完", || LOG_B.lock().unwrap().iter().any(|j| j.contains(r#""t":"recv_done""#)));
        until("甲知道发完了", || LOG_A.lock().unwrap().iter().any(|j| j.contains(r#""t":"send_done""#) && j.contains(r#""ok":true"#)));
        assert!(LOG_B.lock().unwrap().iter().any(|j| j.contains(r#""t":"recv_done""#) && j.contains(r#""ok":true"#) && j.contains(r#""count":2"#)));
        assert_eq!(std::fs::read(base.join("收/照片/大.bin")).unwrap().len(), 3_000_000);
        assert_eq!(std::fs::read_to_string(base.join("收/照片/小.txt")).unwrap(), "你好");
        assert_eq!(ywk_send_files(a, c(&ib).as_ptr(), c("").as_ptr()), 0);
        let _ = std::fs::remove_dir_all(&base);

        // 两边的鼠标动的是同一个指针：甲的鼠标在乙上时，乙动自己的鼠标不会把甲赶走，甲接着从指针现在的地方走
        INPUT_B.lock().unwrap().clear();
        ywk_local_move(b, 500.0, 500.0, 40.0, 0.0);                          // 没有别人的鼠标在乙上：什么都不发生
        cross(a);
        until("乙知道甲进来了", || INPUT_B.lock().unwrap().contains(&(1, 1, 720, 0, 0)));
        ywk_local_move(b, 500.0, 500.0, 40.0, 0.0);                          // 乙自己的鼠标把指针挪到 (500, 500)
        assert_eq!(ywk_key(b, 0x05, 1), 0, "乙自己的键盘照常给乙");
        assert_eq!(ywk_key(b, 0x05, 0), 0);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(ywk_remote(a), 1, "甲没有被赶走");
        ywk_remote_move(a, 10.0, 0.0);
        until("甲接着从乙的指针现在的地方走", || INPUT_B.lock().unwrap().contains(&(1, 510, 500, 0, 0)));

        // 没有主从：乙自己的鼠标推出左边缘（那边是甲），先请甲的鼠标回去，然后乙的鼠标到甲上
        ywk_local_move(b, 0.0, 720.0, -5.0, 0.0);
        until("乙的鼠标穿到甲上", || ywk_remote(b) == 1);
        until("甲的鼠标被请回去", || ywk_remote(a) == 0);
        until("乙替甲松开", || INPUT_B.lock().unwrap().contains(&(6, 1, 0, 0, 0)));
        until("甲知道乙进来了", || INPUT_A.lock().unwrap().contains(&(5, 1, 500, 0, 0)));
        assert_eq!(ywk_key(b, 0x06, 1), 1);
        until("甲收到乙的按键", || INPUT_A.lock().unwrap().contains(&(3, 6, 1, 0, 0)));
        assert_eq!(ywk_key(a, 0x07, 1), 0, "甲自己的键盘照常给甲，也不会把乙赶走");
        assert_eq!(ywk_key(a, 0x07, 0), 0);
        ywk_remote_move(b, 5000.0, 0.0);                                    // 乙的鼠标在甲上一直往右：走到甲的右边缘，甲告诉乙“到边了”，乙回去
        until("乙的鼠标回来", || ywk_remote(b) == 0);
        until("甲知道乙走了", || INPUT_A.lock().unwrap().contains(&(6, 0, 0, 0, 0)));
        assert_eq!(ywk_remote(a), 0);
        // 修饰键的抬起没监听到（比如锁屏时松的）：贴边时按系统说的补上，穿过去不会带着它
        assert_eq!(ywk_key(a, 0xE0, 1), 0);
        ywk_sync_mods(a, 0);
        INPUT_B.lock().unwrap().clear();
        cross(a);
        until("乙知道甲进来了，没带修饰键", || INPUT_B.lock().unwrap().contains(&(5, 0, 500, 0, 0)));
        ywk_remote_move(a, -500.0, 0.0);
        until("甲的鼠标回来", || ywk_remote(a) == 0);
        until("乙知道甲走了", || INPUT_B.lock().unwrap().contains(&(6, 0, 0, 0, 0)));

        cross(a);                                                           // 再穿过去，然后紧急拉回
        ywk_home(a);
        until("紧急拉回", || ywk_remote(a) == 0);
        ywk_home(a);                                                        // 已经在本机：什么都不做
        until("穿走几次就回来几次", || count(&LOG_A, "grab") == 4 && count(&LOG_A, "release") == 4);
        INPUT_B.lock().unwrap().clear();
        cross(a);                                                           // 穿过去的时候控制方掉线：被控方要松开按键
        until("乙知道甲进来了", || INPUT_B.lock().unwrap().iter().any(|i| i.0 == 5));
        let releases = count(&LOG_A, "release");
        ywk_stop(a);
        assert_eq!(count(&LOG_A, "release"), releases + 1, "关掉时鼠标还在对面：先拉回来，外壳收到 release");
        until("控制方关掉后替它松开", || { let i = INPUT_B.lock().unwrap(); i.contains(&(6, 0, 0, 0, 0)) || i.contains(&(6, 2, 0, 0, 0)) });
        // 用存下的摆法重启
        let a = ywk_start(c(&ia).as_ptr(), c("书房的 Mac").as_ptr(), 1, 1920, 1080, c(ka.0).as_ptr(), c(ka.1).as_ptr(), c(&saved_a).as_ptr(), pa, 0, Some(note_a), Some(input_a), std::ptr::null_mut());
        assert!(!a.is_null());
        ywk_restore_layout(a, 9, c(&format!("{ia}:0:0,{ib}:1:0")).as_ptr());
        LOG_A.lock().unwrap().clear();
        ywk_join(a, c("127.0.0.1").as_ptr(), ywk_port(b), c("").as_ptr());
        until("同组的直接连上", || field(&LOG_A, "connected", "id").is_some());
        assert!(field(&LOG_A, "join_failed", "name").is_none() && field(&LOG_A, "group", "code").is_none(), "用存下的组启动：不用再加入，也不用再存");
        ywk_set_size(a, 1280, 800);
        ywk_local_move(a, 1279.0, 400.0, 5.0, 0.0);                          // 屏幕变小以后，新的右边缘照样能穿
        until("按新的屏幕大小穿过去", || ywk_remote(a) == 1);
        ywk_home(a);
        until("回来", || ywk_remote(a) == 0);
        LOG_A.lock().unwrap().clear();
        ywk_forget(a, c(&ib).as_ptr());                                     // 忘掉台式机：断开，摆法里也拿掉
        until("请出去后断开", || field(&LOG_A, "disconnected", "id").is_some());
        assert!(LOG_A.lock().unwrap().iter().any(|j| j.contains(r#""t":"group""#) && j.contains(r#""joined":0"#)), "请出一台以后这一组换校验码");
        assert!(LOG_A.lock().unwrap().iter().any(|j| j.contains(r#""t":"layout""#) && !j.contains(&ib)));
        LOG_A.lock().unwrap().clear();
        ywk_stop(b);
        ywk_stop(a);
        assert_eq!(ywk_usage_from_mac(0), 4);
        assert_eq!(ywk_mac_from_usage(0xE3), 0x37);
        assert_eq!(ywk_mac_from_usage(0x999), -1);
    }
}
