//! 连网：把前面几块接到真正的网络上。
//!
//! 一台电脑上跑一个 `Engine`：
//!   · 在一个 TCP 端口上等别人来连，也可以主动去连别人；连上先握手加密，再互报家门（Hello）。
//!   · 没配对过的，走一遍 6 位码核对，两边用户都点了确认才算数；配对过的直接连上。
//!   · 鼠标键盘走同一个端口号的 UDP（输入通道），剪贴板、位置同步走 TCP。
//!   · 发生了什么（发现了谁、要核对的码、连上了、对方发来的输入……）通过回调告诉外壳。
//!
//!   · 发现：每 2 秒向同一网络广播一个“我在这”的小包（不加密，只有名字、编号、端口）。收到别人的：
//!     配对过的自动去连（两边里编号小的那台去连，免得撞车）；没配对过的告诉外壳，由用户点“加入”。
//!
//! 线程：一个等 TCP 连接，每个连接一个读线程和一个写线程，一个收 UDP，一个补发（有东西等对方确认时每 4 毫秒看一次，
//!   没事就睡着，等别人叫），一个广播。空闲时这些线程加起来每秒只醒十来次。
//!   · 发 TCP 消息只是放进那条连接的发送队列，由它的写线程加密、写出：总锁里从不等网络，
//!     对方不读、队列满了就断开它，不会把整个引擎卡住；一条连接只有一个线程在加密和写，包号不会乱。
//!   · 认人：握手时拿到对方的身份公钥，它自报的编号必须和配对时记下的那对“编号 + 公钥”对得上。

use crate::input::{Action, Endpoint, Event};
use crate::layout::Layout;
use crate::secure::{same, Group, Handshake, Identity, Join, Session};
use crate::wire::{ClipKind, ClipTag, DeviceId, FrameReader, Msg, Os, PROTOCOL};
use std::collections::HashMap;
use crate::files::Incoming;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 47800;

/// 输入通道现在有多忙：0 没事、1 正控制着别的电脑（隔一会儿报个平安）、2 有东西等对方确认（要勤快地补发）。
/// 补发线程按它决定睡多久；变忙的时候把补发线程叫醒
#[derive(Default)]
struct Pace { level: Mutex<u8>, cv: Condvar }

/// 往别的电脑发输入的把手：可以复制、带到别的线程用（接口层的后台线程、引擎的回调里都用它）。
/// 它不攥着网络端口：引擎停了以后再发只是不了了之，端口照常放开
#[derive(Clone)]
pub struct Wire { shared: Arc<Mutex<Shared>>, udp: std::sync::Weak<UdpSocket>, pace: Arc<Pace> }
impl Wire {
    fn flush(&self) { if let Some(udp) = self.udp.upgrade() { flush(&self.shared, &udp, &self.pace, false); } }
    /// 排进一个不能丢的输入事件（按键、点击、穿屏）。返回 false：对方没连上，或者太久没回应
    pub fn event(&self, id: DeviceId, ev: Event) -> bool {
        let ok = { let mut s = self.shared.lock().unwrap(); s.peers.get_mut(&id).map_or(false, |p| p.ready && p.endpoint.push(ev)) };
        self.flush();
        ok
    }
    /// 鼠标又移动了这么多
    pub fn motion(&self, id: DeviceId, dx: i32, dy: i32) {
        if let Some(p) = self.shared.lock().unwrap().peers.get_mut(&id) { p.endpoint.motion(dx, dy) }
        self.flush();
    }
    pub fn wheel(&self, id: DeviceId, dx: i32, dy: i32) {
        if let Some(p) = self.shared.lock().unwrap().peers.get_mut(&id) { p.endpoint.wheel(dx, dy) }
        self.flush();
    }
    /// 开始 / 不再控制对方（控制着的时候隔一会儿报个平安）
    pub fn hold(&self, id: DeviceId, on: bool) { if let Some(p) = self.shared.lock().unwrap().peers.get_mut(&id) { p.endpoint.hold(on) } }
}

#[derive(Clone)]
pub struct Config {
    pub id: DeviceId,
    pub name: String,
    pub os: Os,
    pub width: u32,
    pub height: u32,
    pub identity: Identity,
    /// 这台电脑所在的组（校验码和组钥匙）。同组的电脑发现了就自动连上
    pub group: Group,
    /// TCP 和 UDP 都用这个端口号；0 表示让系统随便给一个（测试用）
    pub port: u16,
    /// 要不要广播自己、让别人发现（要用固定端口 DEFAULT_PORT 才互相找得到）
    pub discover: bool,
}

const BEACON: &[u8; 4] = b"YWB2";
/// 想加入的一方等对方的用户点“允许”，最多等这么久
const JOIN_WAIT: Duration = Duration::from_secs(120);
/// 校验码连着输错这么多次，就这么久不再接受加入（不让人一个一个试）
const JOIN_TRIES: u32 = 5;
const JOIN_LOCK: Duration = Duration::from_secs(60);

/// “我在这”的小包：标记 + 编号 + 端口 + 系统 + 组的记号 + 名字
pub fn beacon(id: &DeviceId, port: u16, os: Os, tag: &[u8; 8], name: &str) -> Vec<u8> {
    let mut b = BEACON.to_vec();
    b.extend_from_slice(id); b.extend_from_slice(&port.to_be_bytes()); b.push(os as u8); b.extend_from_slice(tag);
    let n = name.as_bytes(); let mut len = n.len().min(60);
    while !name.is_char_boundary(len) { len -= 1 }
    b.push(len as u8); b.extend_from_slice(&n[..len]);
    b
}
pub fn parse_beacon(p: &[u8]) -> Option<(DeviceId, u16, Os, [u8; 8], String)> {
    if p.len() < 32 || &p[..4] != BEACON { return None }
    let id: DeviceId = p[4..20].try_into().ok()?;
    let port = u16::from_be_bytes([p[20], p[21]]);
    let os = match p[22] { 1 => Os::Mac, 2 => Os::Windows, 3 => Os::Linux, _ => return None };
    let tag: [u8; 8] = p[23..31].try_into().ok()?;
    let len = p[31] as usize;
    if p.len() != 32 + len || port == 0 { return None }
    Some((id, port, os, tag, String::from_utf8(p[32..].to_vec()).ok()?))
}

/// 告诉外壳发生了什么
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// 同一网络里发现了一台不在本组的电脑：用户点“加入”、输入它的校验码后调 `join(addr, code)`
    Found { id: DeviceId, name: String, os: Os, addr: SocketAddr },
    /// 本机所在的组变了：加入了别人的组（joined，这时原来组里的电脑都断开了，外壳把它们从界面上拿掉），
    /// 或者这一组换了校验码。code 给用户看；save 请原样存起来，下次启动时传回来
    Group { code: String, save: String, joined: bool },
    /// 有一台电脑输对了校验码，想加入这一组：请问用户允不允许，然后调 `confirm_join`
    JoinRequest { id: DeviceId, name: String, os: Os },
    /// 本机想加入别人的组，没成：why 1 校验码不对、2 对方没同意、3 对方暂时不接受加入、4 连不上或者等太久
    JoinFailed { name: String, why: u8 },
    /// 可以用了
    Connected { id: DeviceId, name: String, os: Os, width: u32, height: u32 },
    Disconnected { id: DeviceId },
    /// 发给这台电脑的按键、穿屏一直没得到确认（输入通道不通，多半是它的防火墙挡着）：已经断开它重连，请提示用户
    Stalled { name: String },
    /// 来连的那台电脑上的一苇和本机不是同一版（theirs 是它的协议版本），连不了：请提示用户两边换成同一版
    Mismatch { name: String, theirs: u32 },
    /// 对方的屏幕大小变了
    Resized { id: DeviceId, width: u32, height: u32 },
    /// 对方发来的鼠标键盘：按顺序照做
    Input { id: DeviceId, actions: Vec<Action> },
    /// 对方带来的剪贴板文字（tag 见 wire::ClipTag；不是带回来的话，空的表示它这次没有能带的）
    ClipText { id: DeviceId, text: String, tag: ClipTag },
    /// 对方带来的剪贴板图片，已经存成这个文件（用完请删掉）
    ClipImage { id: DeviceId, path: PathBuf, tag: ClipTag },
    /// 对方带来的复制的文件、文件夹，已经收在临时文件夹里：paths 是最上层的那几个。别删（这台电脑随时可能粘贴）；
    /// 同一台电脑下次再带文件来时，核心会把这一批清掉
    ClipFiles { id: DeviceId, paths: Vec<PathBuf>, tag: ClipTag },
    /// 带给这台电脑的图片、文件半路没带成：外壳别再当成“带过了”，下次指针过去重新带
    ClipFailed { id: DeviceId },
    Layout { rev: u64, cells: Vec<(DeviceId, i16, i16)> },
    /// 发文件的进度、结果（xfer 是 send_files 返回的编号）
    SendProgress { xfer: u32, id: DeviceId, done: u64, total: u64 },
    SendDone { xfer: u32, id: DeviceId, ok: bool, err: String },
    /// 收文件：开始、进度、结果（xfer 是对方的编号；first 是收下的第一个最上层的文件或文件夹）
    RecvBegin { xfer: u32, id: DeviceId, name: String, count: usize, total: u64 },
    RecvProgress { xfer: u32, id: DeviceId, done: u64, total: u64 },
    RecvDone { xfer: u32, id: DeviceId, ok: bool, err: String, first: PathBuf, count: usize, total: u64 },
}

type Callback = Arc<dyn Fn(Notice) + Send + Sync>;

/// 收到的输入先排在这里，由“照做”线程取出来交给外壳。照做（往系统里注入）有时一次要几毫秒，直接在收包线程里做的话，
/// 包一密就越积越多、鼠标越来越滞后（2026-11-13 真机记录：Windows 每秒收 680 个位置，28% 的照做超过 2 毫秒，丢了四分之一）。
/// 排着队的时候，连着的几次鼠标移动并成一次（移动量加起来，一点不少走；Moonlight 的被控端也是这么做的）
type InQ = Arc<(Mutex<VecDeque<(DeviceId, Action)>>, Condvar)>;

struct Peer {
    id: DeviceId,
    name: String,
    os: Os,
    size: (u32, u32),
    tcp: TcpStream,                 // 只用来断开；写由写线程负责
    tx: Outbox,                     // 这条连接的发送队列
    dialer: DeviceId,               // 这条连接是谁拨出的（两边同时连时据此留一条）
    session: Arc<Mutex<Session>>,
    endpoint: Endpoint,
    udp: SocketAddr,
    ready: bool,              // 是同组的，可以收发输入
    join: Option<Join>,       // 正在核对校验码（它想加入这一组，或者本机想加入它那一组）
    asked: bool,              // 它的校验码对了，正等本机的用户点“允许”
    deadline: Option<Instant>,// 还不是组员的连接：到这个时候还没进展就断开
}

struct Shared {
    cfg: Config,
    peers: HashMap<DeviceId, Peer>,
    by_addr: HashMap<SocketAddr, DeviceId>,
    group: Group,
    join_fails: u32,                                // 校验码连着输错了几次
    join_locked: Option<Instant>,                   // 输错太多次：从这时起一段时间不接受加入
    heard: HashMap<DeviceId, Instant>,              // 还没连上的同组电脑：第一次听到它的广播是什么时候
    found: HashMap<DeviceId, (SocketAddr, Instant)>,
    dialing: HashMap<DeviceId, Instant>,
    layout: Layout,
    started: Instant,
    recv_dir: PathBuf,                              // 收到的文件放这里
    next_xfer: u32,
    sending: HashMap<u32, Arc<SendCtl>>,            // 正在发的（编号是本机自己的）
    refuse: HashSet<(DeviceId, u32)>,               // 用户点了“不收了”的
    mismatch: HashMap<std::net::IpAddr, Instant>,   // 版本不一样的电脑：上次什么时候告诉过外壳
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    udp: Arc<UdpSocket>,
    port: u16,
    stop: Arc<AtomicBool>,
    notify: Callback,
    pace: Arc<Pace>,
    slots: Arc<AtomicUsize>,                        // 现在有几条连接
    inq: InQ,
    /// 常驻的几个线程：停止时等它们退出，端口才真正放开，马上重新启动不会撞上
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// 一个引擎同时最多这么多条连接（含正在握手的），免得被人开一大堆连接拖垮
const MAX_CONNECTIONS: usize = 16;
/// 剪贴板文字最多这么长
pub const MAX_CLIP: usize = 4 * 1024 * 1024;
struct Slot(Arc<AtomicUsize>);
impl Slot { fn take(n: &Arc<AtomicUsize>) -> Option<Slot> { if n.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS { n.fetch_sub(1, Ordering::SeqCst); None } else { Some(Slot(n.clone())) } } }
impl Drop for Slot { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); } }

fn now_ms(s: &Shared) -> u64 { s.started.elapsed().as_millis() as u64 }

/// 一次发送的控制：本机要取消、对方的回话
#[derive(Default)]
struct SendCtl { peer: DeviceId, clip: Option<(ClipTag, ClipKind)>, cancel: AtomicBool, answer: Mutex<Option<(bool, String)>>, cv: Condvar }

/// 一条连接的发送队列，分两条：小消息（配对、剪贴板、摆法、心跳……）和文件内容。写线程总是先发小消息，
/// 传大文件时这些照样及时；文件那条只排几块，发文件的线程在这里等着，不会一下子把内存撑满。
#[derive(Default)]
struct Queues { ctrl: VecDeque<Msg>, bulk: VecDeque<Msg>, closed: bool }
#[derive(Clone, Default)]
struct Outbox(Arc<(Mutex<Queues>, Condvar)>);
const CTRL_MAX: usize = 256;
const BULK_MAX: usize = 4;
impl Outbox {
    /// 小消息：排不下（对方一直不读）返回 false
    fn try_send(&self, m: Msg) -> bool {
        let (q, cv) = &*self.0; let mut g = q.lock().unwrap();
        if g.closed || g.ctrl.len() >= CTRL_MAX { return false }
        g.ctrl.push_back(m); cv.notify_all(); true
    }
    /// 文件内容：排满了就等；连接断了返回 false
    fn send_bulk(&self, m: Msg) -> bool {
        let (q, cv) = &*self.0; let mut g = q.lock().unwrap();
        while !g.closed && g.bulk.len() >= BULK_MAX { g = cv.wait(g).unwrap() }
        if g.closed { return false }
        g.bulk.push_back(m); cv.notify_all(); true
    }
    /// 写线程取下一条；关掉了返回 None。每隔 2 秒自己插一个心跳：心跳由写线程发，
    /// 收的一方就算在忙着写磁盘，也照样告诉对方“我还在”
    fn next(&self, last_ping: &mut Instant) -> Option<Msg> {
        let (q, cv) = &*self.0; let mut g = q.lock().unwrap();
        loop {
            if g.closed { return None }
            if last_ping.elapsed() >= Duration::from_secs(2) { *last_ping = Instant::now(); return Some(Msg::Ping { nonce: 0 }) }
            if let Some(m) = g.ctrl.pop_front() { return Some(m) }
            if let Some(m) = g.bulk.pop_front() { cv.notify_all(); return Some(m) }
            let wait = Duration::from_secs(2).saturating_sub(last_ping.elapsed());
            g = cv.wait_timeout(g, wait).unwrap().0;
        }
    }
    fn close(&self) { let (q, cv) = &*self.0; q.lock().unwrap().closed = true; cv.notify_all(); }
    fn closed(&self) -> bool { self.0 .0.lock().unwrap().closed }
}

/// 放进这条连接的发送队列。排不下说明对方一直不读：断开它
fn send_tcp(peer: &mut Peer, msg: &Msg) -> bool {
    if peer.tx.try_send(msg.clone()) { true } else { let _ = peer.tcp.shutdown(std::net::Shutdown::Both); false }
}

/// 握手时的一句话：2 字节长度 + 内容
fn write_hs(s: &mut TcpStream, m: &[u8]) -> std::io::Result<()> { s.write_all(&(m.len() as u16).to_be_bytes())?; s.write_all(m) }
fn read_hs(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 2]; s.read_exact(&mut len)?;
    let mut m = vec![0u8; u16::from_be_bytes(len) as usize]; s.read_exact(&mut m)?;
    Ok(m)
}

impl Engine {
    pub fn start(cfg: Config, notify: impl Fn(Notice) + Send + Sync + 'static) -> std::io::Result<Engine> {
        // 以前带来的剪贴板图片、文件：放了一天以上的清掉（刚带来的不动，剪贴板里可能还指着它）
        if let Ok(dir) = std::fs::read_dir(clip_root()) {
            for e in dir.filter_map(|e| e.ok()) {
                if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map_or(false, |d| d > Duration::from_secs(86400)) { let _ = std::fs::remove_dir_all(e.path()); }
            }
        }
        // 端口传 0（测试用）时让系统挑一个；挑到的号 UDP 那边可能正好被别人占着，换一个再试
        let mut tries = 0;
        let (listener, port, udp) = loop {
            let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, cfg.port))?;
            let port = listener.local_addr()?.port();
            match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)) {
                Ok(u) => break (listener, port, Arc::new(u)),
                Err(e) if cfg.port != 0 || tries >= 5 => return Err(e),
                Err(_) => tries += 1,
            }
        };
        udp.set_read_timeout(Some(Duration::from_millis(500)))?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Mutex::new(Shared {
            recv_dir: std::env::temp_dir().join("一苇收到的文件"), next_xfer: 1, sending: HashMap::new(), refuse: HashSet::new(), mismatch: HashMap::new(),
            group: cfg.group.clone(), join_fails: 0, join_locked: None, heard: HashMap::new(), found: HashMap::new(), dialing: HashMap::new(),
            cfg, peers: HashMap::new(), by_addr: HashMap::new(), layout: Layout::default(), started: Instant::now(),
        }));
        let engine = Engine { shared, udp, port, stop: Arc::new(AtomicBool::new(false)), notify: Arc::new(notify), pace: Arc::new(Pace::default()), slots: Arc::new(AtomicUsize::new(0)), inq: InQ::default(), threads: Mutex::new(Vec::new()) };
        let mut threads = Vec::new();

        // 等别人来连
        { let (sh, st, nf, slots) = (engine.shared.clone(), engine.stop.clone(), engine.notify.clone(), engine.slots.clone());
          threads.push(std::thread::spawn(move || {
              while !st.load(Ordering::Relaxed) {
                  match listener.accept() {
                      Ok((stream, _)) => {
                          let Some(slot) = Slot::take(&slots) else { drop(stream); continue };
                          // macOS 上接进来的连接会沿用监听口的“不等待”模式，要改回来，否则握手时读不到就直接失败
                          let _ = stream.set_nonblocking(false);
                          let (sh, st, nf, sl) = (sh.clone(), st.clone(), nf.clone(), slots.clone());
                          std::thread::spawn(move || { let _slot = slot; run_connection(stream, false, sh, st, nf, port, None, sl) });
                      }
                      Err(_) => std::thread::sleep(Duration::from_millis(250)),
                  }
              }
          })); }
        // 收鼠标键盘
        { let (sh, st, nf, udp, pc, slots, inq) = (engine.shared.clone(), engine.stop.clone(), engine.notify.clone(), engine.udp.clone(), engine.pace.clone(), engine.slots.clone(), engine.inq.clone());
          threads.push(std::thread::spawn(move || {
              crate::urgent_thread();
              let mut buf = [0u8; 1500];
              while !st.load(Ordering::Relaxed) {
                  let Ok((n, from)) = udp.recv_from(&mut buf) else { continue };
                  if let Some((id, tcp_port, os, tag, name)) = parse_beacon(&buf[..n]) {     // 有人在说“我在这”
                      let addr = SocketAddr::new(from.ip(), tcp_port);
                      let (mut dial, mut tell) = (false, false);
                      { let mut s = sh.lock().unwrap();
                        if id != s.cfg.id && !s.peers.get(&id).map_or(false, |p| p.ready) {
                            if same(&tag, &s.group.tag(&id)) {
                                // 同组的：编号小的那台马上去连；编号大的先等 3 秒——对方可能根本收不到本机的广播
                                // （2026-11-13 真机：Windows 上开着代理软件的虚拟网卡，广播发不到局域网），那就由本机去连。刚拨过就等一等
                                let recently = s.dialing.get(&id).map_or(false, |t| t.elapsed() < Duration::from_secs(5));
                                let first = *s.heard.entry(id).or_insert_with(Instant::now);
                                if !recently && (s.cfg.id < id || first.elapsed() > Duration::from_secs(3)) { s.dialing.insert(id, Instant::now()); dial = true }
                            } else if s.found.len() < 32 || s.found.contains_key(&id) {
                                let fresh = s.found.get(&id).map_or(true, |(a, t)| *a != addr || t.elapsed() > Duration::from_secs(30));
                                if fresh { s.found.insert(id, (addr, Instant::now())); tell = true }
                            }
                        } }
                      if dial { dial_to(addr, sh.clone(), st.clone(), nf.clone(), port, &slots, None) }
                      if tell { nf(Notice::Found { id, name, os, addr }) }
                      continue;
                  }
                  let mut out: Option<(DeviceId, Vec<Action>)> = None;
                  { let mut s = sh.lock().unwrap();
                    let Some(id) = s.by_addr.get(&from).copied() else { continue };
                    let Some(p) = s.peers.get_mut(&id) else { continue };
                    if !p.ready { continue }
                    let Ok((_, plain)) = p.session.lock().unwrap().open_datagram(&buf[..n]) else { continue };
                    if let Ok(actions) = p.endpoint.receive(&plain) { if !actions.is_empty() { out = Some((id, actions)) } }
                  }
                  flush(&sh, &udp, &pc, false);                                      // 对方在等确认的话，马上回
                  if let Some((id, actions)) = out {
                      let (q, cv) = &*inq;
                      let mut g = q.lock().unwrap();
                      for a in actions {
                          // 队尾还排着同一台的一次鼠标移动没来得及照做：并进去
                          if let (Action::Motion { dx, dy }, Some((i, Action::Motion { dx: bx, dy: by }))) = (&a, g.back_mut()) {
                              if *i == id { *bx += dx; *by += dy; continue }
                          }
                          g.push_back((id, a));
                      }
                      cv.notify_one();
                  }
              }
          })); }
        // 照做收到的输入
        { let (st, nf, inq) = (engine.stop.clone(), engine.notify.clone(), engine.inq.clone());
          threads.push(std::thread::spawn(move || {
              crate::urgent_thread();
              let (q, cv) = &*inq;
              loop {
                  let batch: Vec<(DeviceId, Action)> = {
                      let mut g = q.lock().unwrap();
                      while g.is_empty() {
                          if st.load(Ordering::Relaxed) { return }
                          g = cv.wait_timeout(g, Duration::from_millis(500)).unwrap().0;
                      }
                      g.drain(..).collect()
                  };
                  let mut it = batch.into_iter().peekable();
                  while let Some((id, a)) = it.next() {
                      let mut actions = vec![a];
                      while let Some((_, a)) = it.next_if(|n| n.0 == id) { actions.push(a) }
                      nf(Notice::Input { id, actions });
                  }
              }
          })); }
        // 补发：有东西等对方确认时每 4 毫秒看一次；只是控制着别的电脑时隔一会儿报个平安；都没有就睡着，变忙时被叫醒
        { let (sh, st, nf, udp, pc) = (engine.shared.clone(), engine.stop.clone(), engine.notify.clone(), engine.udp.clone(), engine.pace.clone());
          threads.push(std::thread::spawn(move || {
              crate::urgent_thread();
              loop {
                  for name in flush(&sh, &udp, &pc, true) { nf(Notice::Stalled { name }) }
                  let g = pc.level.lock().unwrap();
                  if st.load(Ordering::Relaxed) { break }
                  let wait = match *g { 2 => 4, 1 => crate::input::HEARTBEAT_MS / 2, _ => 2000 };
                  let _ = pc.cv.wait_timeout(g, Duration::from_millis(wait)).unwrap();
              }
          })); }
        // 广播“我在这”
        let discover = engine.shared.lock().unwrap().cfg.discover;
        if discover {
            let _ = engine.udp.set_broadcast(true);
            let (st, udp, sh) = (engine.stop.clone(), engine.udp.clone(), engine.shared.clone());
            threads.push(std::thread::spawn(move || {
                while !st.load(Ordering::Relaxed) {
                    // 每次现做：组可能刚换过
                    let hello = { let s = sh.lock().unwrap(); beacon(&s.cfg.id, port, s.cfg.os, &s.group.tag(&s.cfg.id), &s.cfg.name) };
                    let _ = udp.send_to(&hello, (Ipv4Addr::BROADCAST, DEFAULT_PORT));
                    // Windows 只从“最优先”的那块网卡发广播，装了代理软件、虚拟机的电脑上那常常是块虚拟网卡，局域网里谁也收不到：
                    // 每块网卡各发一次（从绑在那块网卡地址上的口子发）
                    for ip in local_ips() {
                        if let Ok(u) = UdpSocket::bind((ip, 0)) { let _ = u.set_broadcast(true); let _ = u.send_to(&hello, (Ipv4Addr::BROADCAST, DEFAULT_PORT)); }
                    }
                    for _ in 0..4 { if st.load(Ordering::Relaxed) { return }; std::thread::sleep(Duration::from_millis(500)) }
                }
            }));
        }
        *engine.threads.lock().unwrap() = threads;
        Ok(engine)
    }

    pub fn port(&self) -> u16 { self.port }

    /// 用户要加入一台电脑所在的组（发现了它点“加入”，或者手动填了地址）：code 是它的校验码。
    /// 同组的电脑平时发现了就自动连，用不着这个；已经同组时调它只是马上去连一次，code 用不上
    pub fn join(&self, addr: SocketAddr, code: &str) {
        dial_to(addr, self.shared.clone(), self.stop.clone(), self.notify.clone(), self.port, &self.slots, Some(code.to_string()))
    }
    /// 有电脑想加入这一组（`Notice::JoinRequest`），用户点了允许或者不允许
    pub fn confirm_join(&self, id: DeviceId, ok: bool) {
        let mut notices = Vec::new();
        { let mut s = self.shared.lock().unwrap();
          let g = s.group.clone();
          let Some(p) = s.peers.get_mut(&id) else { return };
          if p.ready || !p.asked { return }
          p.asked = false;
          let mac = p.join.take().and_then(|j| j.mac(false));
          match (ok, mac) {
              (true, Some(mac)) => {
                  // 把这一组的校验码和组钥匙交给它（这条连接是加密的），从现在起它是组员
                  send_tcp(p, &Msg::JoinGrant { mac, code: g.code, key: g.key, rev: g.rev });
                  p.ready = true; p.deadline = None;
                  let udp = p.udp;
                  notices.push(Notice::Connected { id, name: p.name.clone(), os: p.os, width: p.size.0, height: p.size.1 });
                  crate::ylog!("允许 {} 加入这一组", hex4(&id));
                  s.by_addr.insert(udp, id);
                  s.found.remove(&id);
                  send_layout(&mut s, id);
                  announce(&mut s, id);
              }
              // 不允许：告诉它一声，它收到就自己断开；一秒后还连着就由这边断
              _ => { send_tcp(p, &Msg::JoinDeny { why: 2 }); p.deadline = Some(Instant::now() + Duration::from_secs(1)); }
          }
        }
        for n in notices { (self.notify)(n) }
    }
    /// 本机现在所在的组
    pub fn group(&self) -> Group { self.shared.lock().unwrap().group.clone() }
    /// 这一组换一个校验码（和组钥匙）：连着的组员跟着换；没连着的拿着旧的进不来，要用新码重新加入
    pub fn new_code(&self) {
        let mut notices = Vec::new();
        regroup(&mut self.shared.lock().unwrap(), None, &mut notices);
        for n in notices { (self.notify)(n) }
    }

    /// 往别的电脑发输入的把手（见 `Wire`）。下面四个是同样的事，直接在引擎上调
    pub fn wire(&self) -> Wire { Wire { shared: self.shared.clone(), udp: Arc::downgrade(&self.udp), pace: self.pace.clone() } }
    pub fn event(&self, id: DeviceId, ev: Event) -> bool { self.wire().event(id, ev) }
    pub fn motion(&self, id: DeviceId, dx: i32, dy: i32) { self.wire().motion(id, dx, dy) }
    pub fn wheel(&self, id: DeviceId, dx: i32, dy: i32) { self.wire().wheel(id, dx, dy) }
    pub fn hold(&self, id: DeviceId, on: bool) { self.wire().hold(id, on) }
    /// 剪贴板里的文字带给一台连着的电脑
    pub fn send_clip_text(&self, id: DeviceId, text: &str, tag: ClipTag) -> bool {
        if text.len() > MAX_CLIP { return false }
        self.shared.lock().unwrap().peers.get_mut(&id).map_or(false, |p| p.ready && send_tcp(p, &Msg::ClipText { text: text.to_string(), tag }))
    }
    /// 用户在这台电脑上摆了位置：记下并告诉所有连着的电脑
    pub fn place(&self, id: DeviceId, x: i16, y: i16) {
        let mut s = self.shared.lock().unwrap();
        s.layout.place(id, x, y);
        let msg = Msg::Layout { rev: s.layout.rev, cells: s.layout.cells().to_vec() };
        for p in s.peers.values_mut().filter(|p| p.ready) { send_tcp(p, &msg); }
    }
    /// 用户把一台电脑请出这一组：断开它、从摆法里拿掉（并告诉其它电脑），然后这一组换一个校验码和组钥匙——
    /// 它拿着旧的进不来。当时没连着的其它组员也要用新码重新加入
    pub fn forget(&self, id: DeviceId) {
        let mut notices = Vec::new();
        { let mut s = self.shared.lock().unwrap();
          drop_peer(&mut s, id, &mut notices);
          let rev = s.layout.rev;
          s.layout.remove(&id);
          if s.layout.rev != rev {
              let msg = Msg::Layout { rev: s.layout.rev, cells: s.layout.cells().to_vec() };
              for p in s.peers.values_mut().filter(|p| p.ready && p.id != id) { send_tcp(p, &msg); }
          }
          regroup(&mut s, Some(id), &mut notices);
        }
        for n in notices { (self.notify)(n) }
    }
    /// 收到的文件放哪
    pub fn set_recv_dir(&self, dir: PathBuf) { self.shared.lock().unwrap().recv_dir = dir }
    /// 给一台连着的电脑发文件、文件夹。返回这次的编号（进度和结果用 SendProgress / SendDone 报），没连上返回 None
    pub fn send_files(&self, id: DeviceId, paths: Vec<PathBuf>) -> Option<u32> { self.send(id, paths, None) }
    /// 把剪贴板里的图片（外壳已经存成一个文件）带给一台连着的电脑。发完（或者发不了）这个文件由这里删掉。
    /// 走的是发文件那条路，但两边都不提示、不显示进度
    pub fn send_clip_image(&self, id: DeviceId, path: PathBuf, tag: ClipTag) -> bool {
        let ok = std::fs::metadata(&path).map_or(false, |m| m.is_file() && m.len() > 0 && m.len() <= MAX_CLIP_IMAGE) && self.send(id, vec![path.clone()], Some((tag, ClipKind::Image))).is_some();
        if !ok { let _ = std::fs::remove_file(&path); }
        ok
    }
    /// 把复制的文件、文件夹带给一台连着的电脑（原件不动）。合起来超过 100MB 或 2000 个就不带，返回 Some(false)；
    /// 没连着、或者里面一个文件都没有，返回 None。
    /// 走的是发文件那条路，但两边都不提示、不显示进度，对面收在临时文件夹里交给剪贴板
    pub fn send_clip_files(&self, id: DeviceId, paths: Vec<PathBuf>, tag: ClipTag) -> Option<bool> {
        if !self.shared.lock().unwrap().peers.get(&id).map_or(false, |p| p.ready) { return None }
        match crate::files::fits(&paths, MAX_CLIP_FILES, MAX_CLIP_FILE_COUNT) {
            crate::files::Fit::Nothing => None,
            crate::files::Fit::TooBig => Some(false),
            crate::files::Fit::Yes => self.send(id, paths, Some((tag, ClipKind::Files))).map(|_| true),
        }
    }
    fn send(&self, id: DeviceId, paths: Vec<PathBuf>, clip: Option<(ClipTag, ClipKind)>) -> Option<u32> {
        let (xfer, tx, ctl) = {
            let mut s = self.shared.lock().unwrap();
            let tx = s.peers.get(&id).filter(|p| p.ready)?.tx.clone();
            let xfer = s.next_xfer; s.next_xfer = s.next_xfer.wrapping_add(1).max(1);
            let ctl = Arc::new(SendCtl { peer: id, clip, ..Default::default() });
            s.sending.insert(xfer, ctl.clone());
            (xfer, tx, ctl)
        };
        let (sh, nf) = (self.shared.clone(), self.notify.clone());
        std::thread::spawn(move || {
            let r = send_manifest(&tx, xfer, id, &paths, &ctl, &nf);
            sh.lock().unwrap().sending.remove(&xfer);
            let (ok, err) = match r { Ok(()) => (true, String::new()), Err(e) => (false, e) };
            match clip {
                None => nf(Notice::SendDone { xfer, id, ok, err }),
                Some((tag, kind)) => {
                    if !ok {
                        // 文件的名字算剪贴板的内容，不进运行记录（出错的原因里可能带着路径）
                        if kind == ClipKind::Image { crate::ylog!("剪贴板里的图片没带过去：{err}") } else { crate::ylog!("复制的文件没带过去（读不了、发的时候变了，或者断开了）") }
                        // 对方那边留着的还是上一次带去的：告诉它这次没有能带的；再让外壳忘掉“带过了”，下次指针过去重新带
                        if !tag.back { if let Some(p) = sh.lock().unwrap().peers.get_mut(&id).filter(|p| p.ready) { send_tcp(p, &Msg::ClipText { text: String::new(), tag }); } }
                        nf(Notice::ClipFailed { id });
                    }
                    // 图片是外壳存的临时文件，发完删掉；复制的文件是用户的原件，绝不能动
                    if kind == ClipKind::Image { for p in &paths { let _ = std::fs::remove_file(p); } }
                }
            }
        });
        Some(xfer)
    }
    /// 不发了
    pub fn cancel_send(&self, xfer: u32) { if let Some(c) = self.shared.lock().unwrap().sending.get(&xfer) { c.cancel.store(true, Ordering::Relaxed); c.cv.notify_all() } }
    /// 不收了
    pub fn cancel_recv(&self, id: DeviceId, xfer: u32) { self.shared.lock().unwrap().refuse.insert((id, xfer)); }
    /// 启动时把上次存下的摆法放回来（比现在的新才采用）
    pub fn restore_layout(&self, rev: u64, cells: &[(DeviceId, i16, i16)]) { self.shared.lock().unwrap().layout.merge(rev, cells); }
    /// 本机屏幕大小变了：告诉连着的电脑
    pub fn set_size(&self, width: u32, height: u32) {
        let mut s = self.shared.lock().unwrap();
        s.cfg.width = width; s.cfg.height = height;
        for p in s.peers.values_mut().filter(|p| p.ready) { send_tcp(p, &Msg::Size { width, height }); }
    }
    pub fn layout(&self) -> Layout { self.shared.lock().unwrap().layout.clone() }
    pub fn connected(&self) -> Vec<DeviceId> { self.shared.lock().unwrap().peers.values().filter(|p| p.ready).map(|p| p.id).collect() }

    /// 停下来：断开所有连接，等常驻线程退出（最多半秒多）。别在回调里调
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        { let _g = self.pace.level.lock().unwrap(); self.pace.cv.notify_all(); }       // 补发线程可能正睡着
        { let _g = self.inq.0.lock().unwrap(); self.inq.1.notify_all(); }             // 照做线程也是
        for p in self.shared.lock().unwrap().peers.values() { let _ = p.tcp.shutdown(std::net::Shutdown::Both); }
        let threads = std::mem::take(&mut *self.threads.lock().unwrap());
        for t in threads { let _ = t.join(); }
    }
}

impl Drop for Engine { fn drop(&mut self) { self.stop() } }

/// 发文件的线程：报清单，然后按顺序一块块读、一块块排进队列，最后等对方回话“收好了”
fn send_manifest(tx: &Outbox, xfer: u32, id: DeviceId, paths: &[PathBuf], ctl: &SendCtl, nf: &Callback) -> Result<(), String> {
    const CHUNK: usize = 128 * 1024;
    let clip = ctl.clip.is_some();
    let nf = |n: Notice| if !clip { nf(n) };                                // 剪贴板的图片不报进度
    let gone = || "连接断开了".to_string();
    let m = crate::files::collect(paths).map_err(|e| format!("读不了要发的文件：{e}"))?;
    if m.is_empty() { return Err("没有可发的文件".into()) }
    let total: u64 = m.iter().map(|f| f.2).sum();
    let begin = Msg::FileBegin { xfer, files: m.iter().map(|f| (f.1.clone(), f.2)).collect(), clip: ctl.clip };
    if begin.encode().len() > crate::wire::MAX_FRAME - 1024 { return Err("文件太多，分几次发".into()) }
    if !tx.send_bulk(begin) { return Err(gone()) }
    // 从这里起，本机这边出了任何错都要告诉对方“不发了”，免得它一直等
    let r = (|| -> Result<(), String> {
        let (mut done, mut last) = (0u64, Instant::now());
        nf(Notice::SendProgress { xfer, id, done, total });
        let mut buf = vec![0u8; CHUNK];
        for (path, _, size) in &m {
            let mut f = std::fs::File::open(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
            let mut left = *size;
            while left > 0 {
                if ctl.cancel.load(Ordering::Relaxed) { return Err("取消了".into()) }
                if let Some((false, e)) = ctl.answer.lock().unwrap().clone() { return Err(format!("对方：{e}")) }
                let n = (left as usize).min(CHUNK);
                f.read_exact(&mut buf[..n]).map_err(|e| format!("读 {} 出错：{e}", path.display()))?;
                if !tx.send_bulk(Msg::FileData { xfer, data: buf[..n].to_vec() }) { return Err(gone()) }
                left -= n as u64; done += n as u64;
                if last.elapsed() > Duration::from_millis(200) { last = Instant::now(); nf(Notice::SendProgress { xfer, id, done, total }) }
            }
            if f.metadata().map(|x| x.len()).unwrap_or(*size) != *size { return Err(format!("{} 在发的时候变了，请重发", path.display())) }
        }
        Ok(())
    })();
    if let Err(e) = r {
        if e != gone() && !e.starts_with("对方：") { tx.send_bulk(Msg::FileCancel { xfer }); }
        return Err(e);
    }
    if !tx.send_bulk(Msg::FileEnd { xfer }) { return Err(gone()) }
    nf(Notice::SendProgress { xfer, id, done: total, total });
    // 等对方回话（收全、改好名字之后它会说一声）
    let mut a = ctl.answer.lock().unwrap();
    let end = Instant::now() + Duration::from_secs(120);
    while a.is_none() && !ctl.cancel.load(Ordering::Relaxed) && Instant::now() < end && !tx.closed() {
        a = ctl.cv.wait_timeout(a, Duration::from_millis(500)).unwrap().0;
    }
    match a.clone() { Some((true, _)) => Ok(()), Some((false, e)) => Err(format!("对方：{e}")), None if tx.closed() => Err(gone()), None => Err("对方没回话".into()) }
}

/// 这台电脑各块网卡的 IPv4 地址（只有 Windows 上要用：见广播那里）。靠解析自己的电脑名得到，系统在本机就能答
#[cfg(windows)]
fn local_ips() -> Vec<Ipv4Addr> {
    use std::net::ToSocketAddrs;
    let Ok(name) = std::env::var("COMPUTERNAME") else { return Vec::new() };
    (name.as_str(), 0).to_socket_addrs().map(|it| it.filter_map(|a| match a.ip() { IpAddr::V4(v) if !v.is_loopback() => Some(v), _ => None }).collect()).unwrap_or_default()
}
#[cfg(not(windows))]
fn local_ips() -> Vec<Ipv4Addr> { Vec::new() }

/// 一台电脑刚成为连着的组员：把其他连着的组员在哪告诉它，也把它在哪告诉它们
fn announce(s: &mut Shared, id: DeviceId) {
    let v4 = |a: &SocketAddr| match a.ip() { IpAddr::V4(v) => Some((v.octets(), a.port())), _ => None };
    let Some(new) = s.peers.get(&id).and_then(|p| v4(&p.udp)) else { return };
    let others: Vec<(DeviceId, [u8; 4], u16)> = s.peers.values().filter(|q| q.ready && q.id != id).filter_map(|q| v4(&q.udp).map(|(ip, port)| (q.id, ip, port))).collect();
    if others.is_empty() { return }
    if let Some(p) = s.peers.get_mut(&id) { send_tcp(p, &Msg::Members { list: others }); }
    let msg = Msg::Members { list: vec![(id, new.0, new.1)] };
    for q in s.peers.values_mut().filter(|q| q.ready && q.id != id) { send_tcp(q, &msg); }
}

fn dial_to(addr: SocketAddr, sh: Arc<Mutex<Shared>>, st: Arc<AtomicBool>, nf: Callback, port: u16, slots: &Arc<AtomicUsize>, join: Option<String>) {
    let Some(slot) = Slot::take(slots) else { if join.is_some() { nf(Notice::JoinFailed { name: String::new(), why: 4 }) } return };
    let sl = slots.clone();
    std::thread::spawn(move || {
        let _slot = slot;
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(stream) => run_connection(stream, true, sh, st, nf, port, join, sl),
            Err(_) => if join.is_some() { nf(Notice::JoinFailed { name: String::new(), why: 4 }) },
        }
    });
}

/// 把每台电脑输入通道里该发的包发出去，并记下现在有多忙（见 `Pace`）。
/// check_stall（只有补发线程传 true）：顺便看看有没有哪台一直不确认——有就断开它（读线程随后报 Disconnected，
/// 鼠标在它上面的话就回本机；重连以后输入通道是新的），返回它们的名字
fn flush(shared: &Arc<Mutex<Shared>>, udp: &UdpSocket, pace: &Pace, check_stall: bool) -> Vec<String> {
    let mut s = shared.lock().unwrap();
    let now = now_ms(&s);
    let mut level = 0u8;
    let mut stalled = Vec::new();
    for p in s.peers.values_mut().filter(|p| p.ready) {
        if check_stall && p.endpoint.take_stalled(now) {
            crate::ylog!("发给“{}”的输入 {} 毫秒没得到确认：断开重连", p.name, crate::input::STALL_MS);
            let _ = p.tcp.shutdown(std::net::Shutdown::Both);
            stalled.push(p.name.clone());
            continue;
        }
        if let Some(plain) = p.endpoint.poll(now) {
            let seq = u64::from_be_bytes(plain[..8].try_into().unwrap());
            if let Ok(packet) = p.session.lock().unwrap().seal_datagram(seq, &plain) { let _ = udp.send_to(&packet, p.udp); }
        }
        level = level.max(if p.endpoint.pending() > 0 { 2 } else if p.endpoint.active() { 1 } else { 0 });
    }
    // 还拿着总锁就记：两个线程先后算出来的忙闲，记下的先后也一样，不会被旧的盖掉
    let mut g = pace.level.lock().unwrap();
    if level > *g { pace.cv.notify_all() }
    *g = level;
    stalled
}

/// 一条连接上最多同时收几批
const MAX_RECV: usize = 4;
/// 读网络的线程交给写磁盘线程的东西。Abort 的第二个值：要不要告诉对方“不收了”
enum ToDisk { Data(Vec<u8>), End, Abort(String, bool) }
struct Recv { tx: std::sync::mpsc::SyncSender<ToDisk>, last: Instant }

/// 开始收一批：起一个写磁盘的线程。最多排 32 块（约 4MB），磁盘慢就让网络那边等着
/// 带给剪贴板的图片最大这么多字节（一张 4K 屏的截图不压缩大约 33MB）
const MAX_CLIP_IMAGE: u64 = 64 << 20;
/// 复制的文件、文件夹合起来不超过这么多才跟着指针带过去（更大的用“发送文件”）
pub const MAX_CLIP_FILES: u64 = 100 << 20;
const MAX_CLIP_FILE_COUNT: usize = 2000;
/// 带来的剪贴板图片、文件收在这下面：每台电脑、每个方向、每次带各一个文件夹
fn clip_root() -> PathBuf { std::env::temp_dir().join("一苇剪贴板") }
fn clip_who(id: &DeviceId, tag: ClipTag) -> String { format!("{}-{}-", id.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>(), tag.back as u8) }
/// 这次带来的收在哪。这个名字的文件夹已经在了（对方重开以后“第几次带”从头数，撞上了以前的）：是没人要的旧东西，先清掉
fn clip_dir(id: &DeviceId, tag: ClipTag) -> PathBuf {
    let d = clip_root().join(format!("{}{}", clip_who(id, tag), tag.no));
    let _ = std::fs::remove_dir_all(&d);
    d
}
/// 这一次收全了：同一台电脑同一个方向更早带来的没人要了，清掉。
/// 收全之前不清——没收完、没收成的时候，剪贴板里还指着上一批；只清更早的——晚到的旧一批不能把新一批清了
fn clip_sweep(id: &DeviceId, tag: ClipTag) {
    let who = clip_who(id, tag);
    let Ok(dir) = std::fs::read_dir(clip_root()) else { return };
    for e in dir.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(no) = name.strip_prefix(&who) else { continue };
        if no.parse::<u32>().map_or(true, |n| n < tag.no) { let _ = std::fs::remove_dir_all(e.path()); }
    }
}
fn start_recv(xfer: u32, id: DeviceId, dir: PathBuf, files: Vec<(String, u64)>, kind: Option<(ClipTag, ClipKind)>, out: Outbox, shared: Arc<Mutex<Shared>>, notify: Callback) -> Recv {
    let (tx, rx) = std::sync::mpsc::sync_channel::<ToDisk>(32);
    let clip = kind.is_some();
    std::thread::spawn(move || {
        let fail = |err: String, tell: bool| {
            if tell { let _ = out.try_send(Msg::FileAck { xfer, ok: false, err: err.clone() }); }
            err
        };
        let mut inc = match Incoming::begin(&dir, &files) {
            Ok(i) => i,
            Err(e) => {
                let err = fail(e.to_string(), true);
                if !clip { notify(Notice::RecvDone { xfer, id, ok: false, err, first: dir, count: 0, total: 0 }) }
                return;
            }
        };
        let name = inc.first.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !clip { notify(Notice::RecvBegin { xfer, id, name, count: inc.count, total: inc.total }) }
        let mut last = Instant::now();
        let r: Result<(), String> = loop {
            match rx.recv() {
                Ok(ToDisk::Data(d)) => {
                    if let Err(e) = inc.data(&d) { break Err(fail(format!("存不下：{e}"), true)) }
                    if !clip && last.elapsed() > Duration::from_millis(200) { last = Instant::now(); notify(Notice::RecvProgress { xfer, id, done: inc.got, total: inc.total }) }
                }
                Ok(ToDisk::End) => break match inc.end() {
                    Ok(()) => { let _ = out.try_send(Msg::FileAck { xfer, ok: true, err: String::new() }); Ok(()) }
                    Err(e) => Err(fail(e.to_string(), true)),
                },
                Ok(ToDisk::Abort(why, tell)) => break Err(fail(why, tell)),
                Err(_) => break Err("连接断开了".into()),
            }
        };
        shared.lock().unwrap().refuse.remove(&(id, xfer));
        let (ok, err) = match r { Ok(()) => (true, String::new()), Err(e) => (false, e) };
        let n = match kind {
            Some((tag, ClipKind::Image)) => Notice::ClipImage { id, path: inc.first.clone(), tag },
            Some((tag, ClipKind::Files)) => Notice::ClipFiles { id, paths: inc.tops(), tag },
            None => Notice::RecvDone { xfer, id, ok, err, first: inc.first.clone(), count: inc.count, total: inc.total } };
        let dir = inc.dir().to_path_buf();
        drop(inc);                                                          // 先删掉没收完的临时文件，再报
        if clip && !ok { let _ = std::fs::remove_dir_all(dir); }            // 带剪贴板的没收全：整批不要
        if let (Some((tag, _)), true) = (kind, ok) { clip_sweep(&id, tag) }
        if !clip || ok { notify(n) }
    });
    Recv { tx, last: Instant::now() }
}

/// 当场和一台电脑断开并把它从表里拿掉（不等它那条连接的读线程自己发现）：从这一刻起它发来的东西都不认。
/// 原来是组员、或者正等着用户点“允许”的，告诉外壳它走了
fn drop_peer(s: &mut Shared, id: DeviceId, notices: &mut Vec<Notice>) {
    let Some(p) = s.peers.remove(&id) else { return };
    let _ = p.tcp.shutdown(std::net::Shutdown::Both);
    if s.by_addr.get(&p.udp) == Some(&id) { s.by_addr.remove(&p.udp); }
    if p.ready || p.asked { notices.push(Notice::Disconnected { id }) }
}
/// 本机换到这一组。joined：是加入了别人的组（只带自己：原来连着的电脑除了 keep 都断开，摆法从头来）；
/// 不是的话，只是本来这一组换了校验码
fn adopt(s: &mut Shared, group: Group, keep: Option<DeviceId>, joined: bool, notices: &mut Vec<Notice>) {
    if joined {
        let old: Vec<DeviceId> = s.peers.keys().copied().filter(|i| Some(*i) != keep).collect();
        for i in old { drop_peer(s, i, notices) }
        s.layout = Layout::default();
        if let Some(id) = keep { s.found.remove(&id); }
    }
    notices.push(Notice::Group { code: group.code.clone(), save: group.to_text(), joined });
    s.group = group;
}
/// 这一组换一个校验码和组钥匙，告诉连着的组员（except：正被请出去的那台不告诉）
fn regroup(s: &mut Shared, except: Option<DeviceId>, notices: &mut Vec<Notice>) {
    let Some(rev) = s.group.rev.checked_add(1) else { return };
    let Ok(g) = Group::generate(rev) else { return };
    let msg = Msg::Regroup { code: g.code.clone(), key: g.key, rev: g.rev, gone: except };
    for p in s.peers.values_mut().filter(|p| p.ready && Some(p.id) != except) { send_tcp(p, &msg); }
    crate::ylog!("这一组换了校验码（第 {} 版）", g.rev);
    adopt(s, g, None, false, notices);
}
fn hex4(id: &DeviceId) -> String { id[..4].iter().map(|b| format!("{b:02x}")).collect() }
fn send_layout(s: &mut Shared, id: DeviceId) {
    if s.layout.rev == 0 { return }
    let msg = Msg::Layout { rev: s.layout.rev, cells: s.layout.cells().to_vec() };
    if let Some(p) = s.peers.get_mut(&id) { send_tcp(p, &msg); }
}

/// 一条 TCP 连接从头到尾：握手 → 互报家门 → （配对）→ 一直读到断开
/// （拨号和连接的线程都不拿 UDP 端口：它们可能比引擎多活几秒，拿着的话关了马上再开会撞端口）
/// join_code：用户要加入对方那一组时输入的校验码（只有本机主动去连、并且是用户点的“加入”才有）
fn run_connection(mut stream: TcpStream, initiator: bool, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>, notify: Callback, my_port: u16, join_code: Option<String>, slots: Arc<AtomicUsize>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let identity = shared.lock().unwrap().cfg.identity.clone();
    // 握手：发起方说第一句，一共三句
    let session = (|| -> Option<Session> {
        if initiator {
            let (mut h, m1) = Handshake::start(&identity).ok()?;
            write_hs(&mut stream, &m1).ok()?;
            let m3 = h.hear(&read_hs(&mut stream).ok()?).ok()??;
            write_hs(&mut stream, &m3).ok()?;
            h.finish().ok()
        } else {
            let mut h = Handshake::respond(&identity).ok()?;
            let m2 = h.hear(&read_hs(&mut stream).ok()?).ok()??;
            write_hs(&mut stream, &m2).ok()?;
            if h.hear(&read_hs(&mut stream).ok()?).ok()?.is_some() { return None }
            h.finish().ok()
        }
    })();
    let Some(session) = session else {
        crate::ylog!("握手失败 {:?}", stream.peer_addr());
        if join_code.is_some() { notify(Notice::JoinFailed { name: String::new(), why: 4 }) }
        return;
    };
    let hash = session.hash();
    let session = Arc::new(Mutex::new(session));
    // 写线程：按顺序加密、写出。写不出去（对方不读、断网）5 秒就算断开
    let tx = Outbox::default();
    { let Ok(mut out) = stream.try_clone() else { return };
      let _ = out.set_write_timeout(Some(Duration::from_secs(15)));
      let (session, ob) = (session.clone(), tx.clone());
      std::thread::spawn(move || {
          let mut last_ping = Instant::now();
          while let Some(msg) = ob.next(&mut last_ping) {
              let Ok(sealed) = session.lock().unwrap().seal(&msg.encode()) else { break };
              if out.write_all(&sealed).is_err() { break }
          }
          ob.close();
          let _ = out.shutdown(std::net::Shutdown::Both);
      }); }
    // 这条连接上正在收的文件：每批一个写磁盘的线程，读网络的这个线程只把内容转过去，自己不碰磁盘
    let mut incoming: HashMap<u32, Recv> = HashMap::new();
    let hello = { let s = shared.lock().unwrap();
        Msg::Hello { protocol: PROTOCOL, id: s.cfg.id, os: s.cfg.os, name: s.cfg.name.clone(), width: s.cfg.width, height: s.cfg.height, port: my_port,
                     proof: s.group.proof(&hash, initiator) } };
    if !tx.try_send(hello) { return }

    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut frames = FrameReader::default();
    let mut buf = [0u8; 16384];
    let mut me: Option<DeviceId> = None;
    let (mut last_heard, born) = (Instant::now(), Instant::now());
    // guest：对方还不是组员（正在申请加入，或者本机正在申请加入它）。was_member：这条连接上对方成了组员。
    // told：加入没成已经告诉过外壳了。their_name：对方报的名字
    let (mut guest, mut was_member, mut told, mut their_name) = (true, false, false, String::new());
    'conn: while !stop.load(Ordering::Relaxed) {
        if last_heard.elapsed() > Duration::from_secs(7) { break }                 // 对方没声音了
        if me.is_none() && born.elapsed() > Duration::from_secs(10) { break }      // 连上了却迟迟不报家门
        if let (true, Some(id)) = (guest, me) {
            // 还不是组员的连接不能一直挂着：到时候没进展就断开
            let s = shared.lock().unwrap();
            match s.peers.get(&id).filter(|p| Arc::ptr_eq(&p.session, &session)) {
                Some(p) if p.ready => guest = false,
                Some(p) => if p.deadline.map_or(false, |t| Instant::now() > t) { break },
                None => break,
            }
        }
        if let (false, Some(id)) = (incoming.is_empty(), me) {
            // 用户点了“不收了”、对方很久没发数据：结束那一批
            let refuse: Vec<u32> = { let mut s = shared.lock().unwrap(); incoming.keys().copied().filter(|x| s.refuse.remove(&(id, *x))).collect() };
            for x in refuse { if let Some(r) = incoming.remove(&x) { let _ = r.tx.send(ToDisk::Abort("取消了".into(), true)); } }
            let idle: Vec<u32> = incoming.iter().filter(|(_, r)| r.last.elapsed() > Duration::from_secs(60)).map(|(x, _)| *x).collect();
            for x in idle { if let Some(r) = incoming.remove(&x) { let _ = r.tx.send(ToDisk::Abort("对方一分钟没发数据".into(), true)); } }
        }
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => break,
        };
        last_heard = Instant::now();
        let Ok(plain) = session.lock().unwrap().open(&buf[..n]) else { break };
        frames.push(&plain);
        loop {
            let body = match frames.next() { Ok(Some(b)) => b, Ok(None) => break, Err(_) => break 'conn };
            let Ok(msg) = Msg::decode(&body) else { break 'conn };
            let mut notices = Vec::new();
            let mut bye = false;
            let mut dials: Vec<SocketAddr> = Vec::new();
            match msg {
                Msg::Hello { protocol, id, os, name, width, height, port, proof } => {
                    if me.is_some() { break 'conn }
                    let Ok(addr) = stream.peer_addr() else { break 'conn };
                    if protocol != PROTOCOL {
                        // 两边的一苇不是同一版：连不了。告诉外壳去提示用户（同一个地址一分钟最多说一次）
                        let tell = { let mut s = shared.lock().unwrap();
                            let t = s.mismatch.get(&addr.ip()).map_or(true, |t| t.elapsed() > Duration::from_secs(60));
                            if t { s.mismatch.insert(addr.ip(), Instant::now()); }
                            t };
                        if tell {
                            crate::ylog!("“{name}”的协议是第 {protocol} 版，本机是第 {PROTOCOL} 版：连不了");
                            notify(Notice::Mismatch { name, theirs: protocol as u32 });
                        }
                        break 'conn;
                    }
                    let Ok(tcp) = stream.try_clone() else { break 'conn };
                    let udp_addr = SocketAddr::new(addr.ip(), port);
                    let mut s = shared.lock().unwrap();
                    if id == s.cfg.id { break 'conn }                                 // 连到自己了
                    their_name = name.clone();
                    // 是不是同一组的：对方出示的证明，用本机的组钥匙算得出来才是
                    let member = same(&proof, &s.group.proof(&hash, !initiator));
                    let dialer = if initiator { s.cfg.id } else { id };
                    if let Some(old) = s.peers.get(&id) {
                        // 已经有一条连接了。同组的：两边同时拨号时留编号小的那台拨出的那条，同一边又拨了一次（对方重启过）
                        // 就换成新的。不是同组的（来申请加入的）：不能顶掉组员的连接，也不能顶掉正等用户答复的那次申请；
                        // 顶掉的只能是上一次没成的申请
                        let low = s.cfg.id.min(id);
                        let keep_old = if member { old.ready && old.dialer != dialer && old.dialer == low } else { old.ready || old.asked };
                        if keep_old { was_member = member; break 'conn }                  // 已经连着的同组电脑：不算“没加成”
                        let old = s.peers.remove(&id).unwrap();
                        let _ = old.tcp.shutdown(std::net::Shutdown::Both);
                        // 旧连接上的输入通道作废了：先报“断开”，让上面把“谁在控制谁”清干净，再报“连上”
                        if old.ready || old.asked { notices.push(Notice::Disconnected { id }) }
                        if s.by_addr.get(&old.udp) == Some(&id) { s.by_addr.remove(&old.udp); }
                    }
                    // 校验码是 8 位的；没填、填短了的不用去问对方（不然冒充的电脑用空的校验码就能把这台拉进它的组）
                    if let (false, Some(code)) = (member, &join_code) {
                        if Group::normalize(code).len() < 8 { drop(s); told = true; notify(Notice::JoinFailed { name, why: 1 }); break 'conn }
                    }
                    let mut peer = Peer { id, name: name.clone(), os, size: (width, height), tcp, tx: tx.clone(), dialer, session: session.clone(), endpoint: Endpoint::new(),
                                          udp: udp_addr, ready: member, join: None, asked: false, deadline: None };
                    if member { was_member = true; guest = false; notices.push(Notice::Connected { id, name, os, width, height }) }
                    else if let Some(code) = &join_code {
                        // 本机是来加入它这一组的：开始核对校验码
                        let (j, msg) = Join::start(code, &hash, true);
                        peer.join = Some(j); peer.deadline = Some(Instant::now() + Duration::from_secs(10));
                        send_tcp(&mut peer, &Msg::JoinAsk { msg });
                    }
                    else if initiator { break 'conn }                                 // 以为是同组的才去连，它已经不是了
                    else { peer.deadline = Some(Instant::now() + Duration::from_secs(10)) }   // 也许是来加入的：等它开口
                    if member { s.by_addr.insert(udp_addr, id); }                     // 成了组员才收它的鼠标键盘
                    s.peers.insert(id, peer);
                    me = Some(id);
                    if member { s.heard.remove(&id); send_layout(&mut s, id); announce(&mut s, id) }
                }
                other => {
                    let Some(id) = me else { break 'conn };                           // 还没报家门就说别的：不理
                    if let Msg::Ping { nonce } = other { if !tx.try_send(Msg::Pong { nonce }) { break 'conn }; continue }
                    if matches!(other, Msg::FileBegin { .. } | Msg::FileData { .. } | Msg::FileEnd { .. } | Msg::FileCancel { .. } | Msg::FileAck { .. }) {
                        let (ok, dir) = { let s = shared.lock().unwrap();
                            (s.peers.get(&id).map_or(false, |p| p.ready && Arc::ptr_eq(&p.session, &session)), s.recv_dir.clone()) };
                        if !ok { break 'conn }
                        match other {
                            Msg::FileAck { xfer, ok, err } => {
                                // 对方对本机发过去的那批回话：只认真的是发给它的那批
                                let s = shared.lock().unwrap();
                                if let Some(c) = s.sending.get(&xfer).filter(|c| c.peer == id) { *c.answer.lock().unwrap() = Some((ok, err)); c.cv.notify_all() }
                            }
                            Msg::FileBegin { xfer, files, clip: kind } => {
                                let why = if incoming.contains_key(&xfer) { Some("编号重复") } else if incoming.len() >= MAX_RECV { Some("同时收的太多了，等一会儿再发") }
                                          else { match kind {
                                              Some((_, ClipKind::Image)) if files.len() != 1 || files[0].1 > MAX_CLIP_IMAGE => Some("剪贴板的图片太大"),
                                              Some((_, ClipKind::Files)) if files.len() > MAX_CLIP_FILE_COUNT || files.iter().fold(0u64, |a, f| a.saturating_add(f.1)) > MAX_CLIP_FILES => Some("剪贴板的文件太大"),
                                              _ => None } };
                                // 带剪贴板的收在临时文件夹里，不进“收到的文件”
                                let dir = match (kind, why) { (Some((tag, _)), None) => clip_dir(&id, tag), _ => dir };
                                match why {
                                    Some(e) => { let _ = tx.try_send(Msg::FileAck { xfer, ok: false, err: e.into() }); }
                                    None => { incoming.insert(xfer, start_recv(xfer, id, dir, files, kind, tx.clone(), shared.clone(), notify.clone())); }
                                }
                            }
                            Msg::FileData { xfer, data } => if let Some(r) = incoming.get_mut(&xfer) {
                                r.last = Instant::now();
                                // 写磁盘那边排满了就在这里等着（对方也就慢下来）；那边已经结束了就不再转
                                if r.tx.send(ToDisk::Data(data)).is_err() { incoming.remove(&xfer); }
                            },
                            Msg::FileEnd { xfer } => if let Some(r) = incoming.remove(&xfer) { let _ = r.tx.send(ToDisk::End); },
                            Msg::FileCancel { xfer } => if let Some(r) = incoming.remove(&xfer) { let _ = r.tx.send(ToDisk::Abort("对方取消了".into(), false)); },
                            _ => {}
                        }
                        continue;
                    }
                    let mut s = shared.lock().unwrap();
                    // 这条连接已经被同一台电脑的新连接替换掉了
                    if !s.peers.get(&id).map_or(false, |p| Arc::ptr_eq(&p.session, &session)) { break 'conn }
                    match other {
                        Msg::JoinAsk { msg } => {
                            // 它想加入这一组。只有来连本机的、还不是组员的才能申请，一条连接只能申请一次
                            let busy = s.join_locked.map_or(false, |t| t.elapsed() < JOIN_LOCK) || s.peers.values().filter(|p| p.asked).count() >= 3;
                            let code = s.group.code.clone();
                            let Some(p) = s.peers.get_mut(&id) else { break 'conn };
                            if initiator || p.ready || p.join.is_some() { break 'conn }
                            let (mut j, reply) = Join::start(&code, &hash, false);
                            if busy { j.spend(); p.join = Some(j); send_tcp(p, &Msg::JoinDeny { why: 3 }); p.deadline = Some(Instant::now() + Duration::from_secs(1)); continue }
                            if j.finish(&msg).is_err() { break 'conn }
                            p.join = Some(j); p.deadline = Some(Instant::now() + Duration::from_secs(10));
                            send_tcp(p, &Msg::JoinReply { msg: reply });
                        }
                        Msg::JoinReply { msg } => {
                            let Some(p) = s.peers.get_mut(&id) else { break 'conn };
                            if join_code.is_none() || p.ready { break 'conn }
                            let Some(j) = p.join.as_mut() else { break 'conn };
                            if j.finish(&msg).is_err() { break 'conn }                     // 每一步只能走一次
                            let Some(mac) = j.mac(true) else { break 'conn };
                            p.deadline = Some(Instant::now() + JOIN_WAIT);                // 接下来等对方的用户点“允许”
                            send_tcp(p, &Msg::JoinProve { mac });
                        }
                        Msg::JoinProve { mac } => {
                            let busy = s.join_locked.map_or(false, |t| t.elapsed() < JOIN_LOCK);
                            let Some(p) = s.peers.get_mut(&id) else { break 'conn };
                            if initiator || p.ready || p.asked { break 'conn }
                            let Some(mine) = p.join.as_ref().and_then(|j| j.mac(true)) else { break 'conn };
                            if busy { if let Some(j) = p.join.as_mut() { j.spend() } send_tcp(p, &Msg::JoinDeny { why: 3 }); p.deadline = Some(Instant::now() + Duration::from_secs(1)); continue }
                            if same(&mine, &mac) {
                                p.asked = true; p.deadline = Some(Instant::now() + JOIN_WAIT);
                                let (name, os) = (p.name.clone(), p.os);
                                s.join_fails = 0;
                                notices.push(Notice::JoinRequest { id, name, os });
                            } else {
                                // 校验码不对。连着错几次就歇一会儿，不让人一个一个试
                                if let Some(j) = p.join.as_mut() { j.spend() }
                                send_tcp(p, &Msg::JoinDeny { why: 1 }); p.deadline = Some(Instant::now() + Duration::from_secs(1));
                                s.join_fails += 1;
                                crate::ylog!("{} 想加入，校验码不对（连着第 {} 次）", hex4(&id), s.join_fails);
                                if s.join_fails >= JOIN_TRIES { s.join_fails = 0; s.join_locked = Some(Instant::now()); crate::ylog!("校验码输错太多次：一分钟内不接受加入") }
                            }
                        }
                        Msg::JoinGrant { mac, code, key, rev } => {
                            // 对方的用户点了“允许”。它也得证明知道校验码，才收下它给的组钥匙
                            let Some(p) = s.peers.get_mut(&id) else { break 'conn };
                            let ok = join_code.is_some() && !p.ready && p.join.as_ref().and_then(|j| j.mac(false)).map_or(false, |m| same(&m, &mac));
                            if !ok || rev == u64::MAX || code.is_empty() || Group::normalize(&code) != code { break 'conn }
                            p.ready = true; p.join = None; p.deadline = None;
                            was_member = true; guest = false;
                            let (name, os, size, udp) = (p.name.clone(), p.os, p.size, p.udp);
                            s.by_addr.insert(udp, id);
                            crate::ylog!("加入了 {} 所在的组", hex4(&id));
                            adopt(&mut s, Group { code, key, rev }, Some(id), true, &mut notices);
                            notices.push(Notice::Connected { id, name, os, width: size.0, height: size.1 });
                        }
                        Msg::JoinDeny { why } => {
                            if join_code.is_some() && !was_member { notices.push(Notice::JoinFailed { name: their_name.clone(), why }); told = true }
                            bye = true;
                        }
                        Msg::ClipText { text, tag } => { if s.peers.get(&id).map_or(false, |p| p.ready) { notices.push(Notice::ClipText { id, text, tag }) } }
                        Msg::Layout { rev, cells } => {
                            if s.peers.get(&id).map_or(false, |p| p.ready) && s.layout.merge(rev, &cells) {
                                notices.push(Notice::Layout { rev, cells: cells.clone() });
                                let msg = Msg::Layout { rev, cells };                 // 转告其它连着的电脑
                                for p in s.peers.values_mut().filter(|p| p.ready && p.id != id) { send_tcp(p, &msg); }
                            }
                        }
                        Msg::Members { list } => {
                            // 组员转告的“组里别的电脑在哪”：还没连上的，编号小的一方去连（对方也会收到本机在哪，谁小谁去）。
                            // 这里只是地址，去连了以后对方照样得出示组钥匙的证明
                            if !s.peers.get(&id).map_or(false, |p| p.ready) { break 'conn }
                            for (mid, ip, port) in list {
                                if mid == s.cfg.id || port == 0 || s.cfg.id > mid || s.peers.get(&mid).map_or(false, |p| p.ready) { continue }
                                if s.dialing.get(&mid).map_or(false, |t| t.elapsed() < Duration::from_secs(5)) { continue }
                                s.dialing.insert(mid, Instant::now());
                                dials.push(SocketAddr::from((ip, port)));
                            }
                        }
                        Msg::Regroup { code, key, rev, gone } => {
                            // 这一组换了校验码：只听组员的，只跟着更新的换（两台同时换的时候，最后都落在同一个上）
                            if !s.peers.get(&id).map_or(false, |p| p.ready) || code.is_empty() || Group::normalize(&code) != code { break 'conn }
                            // 版本号一下子大出一大截的不认：不然一个要被请出去的组员先发个顶到头的版本号，这一组就再也换不了码
                            if gone == Some(s.cfg.id) || rev > s.group.rev.saturating_add(1 << 32) { continue }
                            if (rev, key) > (s.group.rev, s.group.key) {
                                // 有一台被请出去了：本机也和它断开，换码的事不告诉它
                                if let Some(g) = gone { drop_peer(&mut s, g, &mut notices) }
                                let msg = Msg::Regroup { code: code.clone(), key, rev, gone };
                                adopt(&mut s, Group { code, key, rev }, None, false, &mut notices);
                                for q in s.peers.values_mut().filter(|q| q.ready && q.id != id) { send_tcp(q, &msg); }
                            } else if let (Some(g), true) = (gone, (rev, key) != (s.group.rev, s.group.key)) {
                                // 两台同时换、这一条输了，但它是来请人出去的：请出照做，在现在这一组上再换一次（不告诉被请出去的那台）
                                drop_peer(&mut s, g, &mut notices);
                                regroup(&mut s, Some(g), &mut notices);
                            }
                        }
                        Msg::Size { width, height } => {
                            if let Some(p) = s.peers.get_mut(&id).filter(|p| p.ready) { p.size = (width, height); notices.push(Notice::Resized { id, width, height }) }
                        }
                        _ => {}
                    }
                }
            }
            for n in notices { notify(n) }
            for a in dials { dial_to(a, shared.clone(), stop.clone(), notify.clone(), my_port, &slots, None) }
            if bye { break 'conn }
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    tx.close();
    crate::ylog!("连接结束 {:?}", me.map(|m| m[..4].iter().map(|b| format!("{b:02x}")).collect::<String>()));
    for (_, r) in incoming.drain() { let _ = r.tx.send(ToDisk::Abort("连接断开了".into(), false)); }
    if let Some(id) = me {
        // 是组员、或者正等着用户点“允许”的：告诉外壳它走了（外壳把那个问话的框关掉）
        let tell = { let mut s = shared.lock().unwrap();
            let mine = s.peers.get(&id).map_or(false, |p| Arc::ptr_eq(&p.session, &session));
            if mine { let p = s.peers.remove(&id).unwrap(); if s.by_addr.get(&p.udp) == Some(&id) { s.by_addr.remove(&p.udp); } p.ready || p.asked } else { false } };
        if tell { notify(Notice::Disconnected { id }) }
    }
    // 用户点了“加入”，结果没加成、也没人说为什么（连不上、对方没答复）
    if join_code.is_some() && !was_member && !told { notify(Notice::JoinFailed { name: their_name, why: 4 }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Side;
    use std::sync::mpsc::{channel, Receiver};

    /// 测试里“已经是一组”的电脑共用的组。engine 的第三个参数不空，就表示这台在这一组里（里面写什么无所谓）
    fn team() -> Group { Group { code: "TEAM2345".into(), key: [42; 32], rev: 1 } }
    fn connected(rx: &Receiver<Notice>, who: u8) { wait(rx, |n| match n { Notice::Connected { id, .. } if id == [who; 16] => Some(()), _ => None }) }
    /// 甲用乙的校验码申请加入，等乙那边问用户
    fn ask(a: &Engine, b: &Engine, rb: &Receiver<Notice>) {
        a.join(local(b), &b.group().code);
        wait(rb, |n| matches!(n, Notice::JoinRequest { .. }).then_some(()));
    }
    fn engine(n: u8, name: &str, trusted: Vec<(DeviceId, [u8; 32])>, identity: Option<Identity>) -> (Engine, Receiver<Notice>, Identity) {
        let identity = identity.unwrap_or_else(|| Identity::generate().unwrap());
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let cfg = Config { id: [n; 16], name: name.into(), os: if n == 1 { Os::Mac } else { Os::Windows }, width: 1920, height: 1080, identity: identity.clone(), group: if trusted.is_empty() { Group::generate(1).unwrap() } else { team() }, port: 0, discover: false };
        let e = Engine::start(cfg, move |n| { let _ = tx.lock().unwrap().send(n); }).unwrap();
        (e, rx, identity)
    }
    fn wait<T>(rx: &Receiver<Notice>, mut pick: impl FnMut(Notice) -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let n = rx.recv_timeout(left).expect("等了 5 秒没等到");
            if let Some(v) = pick(n) { return v }
        }
    }
    fn local(e: &Engine) -> SocketAddr { SocketAddr::from(([127, 0, 0, 1], e.port())) }

    #[test]
    fn 两台电脑从配对到互相控制() {
        let (a, ra, ida) = engine(1, "书房的 Mac", vec![], None);
        let (b, rb, idb) = engine(2, "台式机", vec![], None);
        let code_b = b.group().code;
        // 甲要加入乙那一组：输入乙的校验码（小写、带横线也行），乙这边问用户
        a.join(local(&b), &format!("{}-{}", code_b[..4].to_lowercase(), &code_b[4..]));
        let (id, name) = wait(&rb, |n| match n { Notice::JoinRequest { id, name, .. } => Some((id, name)), _ => None });
        assert_eq!((id, name.as_str()), ([1; 16], "书房的 Mac"));
        assert!(!a.event([2; 16], Event::Leave), "对方没允许之前不能发输入");
        assert!(a.connected().is_empty() && b.connected().is_empty());
        b.confirm_join([1; 16], true);
        // 甲改用乙那一组的校验码和组钥匙，并让外壳存下来
        let (code, save) = wait(&ra, |n| match n { Notice::Group { code, save, joined } => { assert!(joined); Some((code, save)) } _ => None });
        assert_eq!(code, code_b);
        assert_eq!(Group::from_text(&save), Some(b.group()));
        assert_eq!(a.group(), b.group());
        connected(&ra, 2); connected(&rb, 1);
        let _ = (ida, idb);

        // 甲控制乙：穿过去、动鼠标、按键、点击、滚轮
        let sent = vec![
            Event::Enter { side: Side::Left, permille: 300, mods: 0, epoch: 1 },
            Event::Key { usage: 0x04, down: true }, Event::Key { usage: 0x04, down: false },
            Event::Button { button: 1, down: true }, Event::Button { button: 1, down: false },
        ];
        for e in &sent { assert!(a.event([2; 16], e.clone())) }
        a.wheel([2; 16], 0, -5);
        a.motion([2; 16], 600, 300); a.motion([2; 16], 40, 60);
        let (mut got, mut wheel, mut pointer) = (Vec::new(), 0, (0, 0));
        while got.len() < sent.len() || wheel != -5 || pointer != (640, 360) {
            for act in wait(&rb, |n| match n { Notice::Input { actions, .. } => Some(actions), _ => None }) {
                match act { Action::Event(e) => got.push(e), Action::Wheel { dy, .. } => wheel += dy, Action::Motion { dx, dy } => pointer = (pointer.0 + dx, pointer.1 + dy) }
            }
        }
        assert_eq!(got, sent);

        // 剪贴板文字、位置同步
        assert!(b.send_clip_text([1; 16], "从台式机复制的字", ClipTag { back: true, no: 3 }));
        assert_eq!(wait(&ra, |n| match n { Notice::ClipText { text, tag, .. } => Some((text, tag)), _ => None }), ("从台式机复制的字".to_string(), ClipTag { back: true, no: 3 }));
        assert!(b.send_clip_text([1; 16], "", ClipTag { back: false, no: 4 }), "空的也要送到：意思是这次没有能带的");
        assert_eq!(wait(&ra, |n| match n { Notice::ClipText { text, tag, .. } => Some((text, tag)), _ => None }), (String::new(), ClipTag { back: false, no: 4 }));
        a.place([1; 16], 0, 0); a.place([2; 16], 1, 0);
        wait(&rb, |n| match n { Notice::Layout { cells, .. } if cells.len() == 2 => Some(()), _ => None });
        assert_eq!(b.layout(), a.layout());

        // 一边退出，另一边会知道
        b.stop();
        wait(&ra, |n| matches!(n, Notice::Disconnected { .. }).then_some(()));
    }

    #[test]
    fn 同组的直接连上() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        let (b, rb, _) = engine(2, "PC", vec![([1; 16], ida.public)], Some(idb));
        b.join(local(&a), "");
        wait(&ra, |n| match n { Notice::Connected { name, .. } => { assert_eq!(name, "PC"); Some(()) } Notice::JoinRequest { .. } => panic!("同组的不该再问"), _ => None });
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        assert!(b.event([1; 16], Event::Key { usage: 0x2C, down: true }));
        let acts = wait(&ra, |n| match n { Notice::Input { actions, .. } => Some(actions), _ => None });
        assert_eq!(acts, vec![Action::Event(Event::Key { usage: 0x2C, down: true })]);
    }

    #[test]
    fn 发现不在本组的电脑会告诉外壳() {
        let (a, ra, _) = engine(1, "Mac", vec![], None);
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.send_to(&beacon(&[9; 16], 4321, Os::Linux, &[0; 8], "客厅的小主机"), local(&a)).unwrap();
        let (name, addr) = wait(&ra, |n| match n { Notice::Found { id, name, os, addr } => { assert_eq!((id, os), ([9; 16], Os::Linux)); Some((name, addr)) } _ => None });
        assert_eq!(name, "客厅的小主机");
        assert_eq!(addr.port(), 4321);
        probe.send_to(&beacon(&[1; 16], 4321, Os::Mac, &[0; 8], "自己"), local(&a)).unwrap();        // 自己的广播不算
        probe.send_to(&[b'Y', b'W', b'B', b'2', 1, 2], local(&a)).unwrap();                  // 残缺的不理
        assert!(ra.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(parse_beacon(&beacon(&[7; 16], 47800, Os::Windows, &[5; 8], "名字很长".repeat(30).as_str())).unwrap().4.chars().count(), 20);
    }

    #[test]
    fn 发现同组的电脑会自动连上() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let mk = |n: u8, id: &Identity, trust: [u8; 32], known: u8| {
            let (tx, rx) = channel(); let tx = Mutex::new(tx);
            let cfg = Config { id: [n; 16], name: format!("电脑{n}"), os: Os::Mac, width: 1920, height: 1080, identity: id.clone(), group: { let _ = (trust, known); team() }, port: 0, discover: false };
            (Engine::start(cfg, move |n| { let _ = tx.lock().unwrap().send(n); }).unwrap(), rx)
        };
        let (a, ra) = mk(1, &ida, idb.public, 2);
        let (b, rb) = mk(2, &idb, ida.public, 1);
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        // 编号大的那台听到广播不动，等对方来连
        probe.send_to(&beacon(&[1; 16], a.port(), Os::Mac, &team().tag(&[1; 16]), "电脑1"), local(&b)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(b.connected().is_empty());
        // 编号小的那台听到广播就去连
        probe.send_to(&beacon(&[2; 16], b.port(), Os::Mac, &team().tag(&[2; 16]), "电脑2"), local(&a)).unwrap();
        wait(&ra, |n| match n { Notice::Connected { .. } => Some(()), Notice::Found { .. } | Notice::JoinRequest { .. } => panic!("同组的不该再问"), _ => None });
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
    }

    #[test]
    fn 第三台找组里任意一台加入_和所有组员自动连上() {
        // 甲、乙已经是一组；丙找乙加入（乙同意），之后丙和甲之间谁也不用点
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        a.join(local(&b), "");
        connected(&ra, 2); connected(&rb, 1);
        let (c, rc, _) = engine(3, "丙", vec![], None);
        ask(&c, &b, &rb);
        b.confirm_join([3; 16], true);
        wait(&rc, |n| match n { Notice::Group { joined, .. } => { assert!(joined); Some(()) } _ => None });
        connected(&rc, 2); connected(&rb, 3);
        assert_eq!(c.group(), team());
        // 甲听到丙的广播（带着本组的记号）：编号小的甲去连，直接连上
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.send_to(&beacon(&[3; 16], c.port(), Os::Windows, &team().tag(&[3; 16]), "丙"), local(&a)).unwrap();
        wait(&ra, |n| match n { Notice::Connected { id, .. } if id == [3; 16] => Some(()), Notice::JoinRequest { .. } | Notice::Found { .. } => panic!("同组的不该再问"), _ => None });
        wait(&rc, |n| match n { Notice::Connected { id, .. } if id == [1; 16] => Some(()), Notice::JoinRequest { .. } => panic!("同组的不该再问"), _ => None });
        assert_eq!((a.connected().len(), b.connected().len(), c.connected().len()), (2, 2, 2));
    }

    #[test]
    fn 听不到广播的组员_靠别的组员转告也能连上() {
        // 甲、乙、丙同组。乙和丙互相听不到广播（这里干脆谁也不广播），各自只连上了甲：甲把它们在哪互相转告，编号小的乙去连丙
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        let (c, rc, _) = engine(3, "丙", vec![([0; 16], [0; 32])], None);
        b.join(local(&a), ""); connected(&rb, 1); connected(&ra, 2);
        c.join(local(&a), ""); connected(&rc, 1); connected(&ra, 3);
        connected(&rb, 3); connected(&rc, 2);
        assert_eq!((a.connected().len(), b.connected().len(), c.connected().len()), (2, 2, 2));
    }

    #[test]
    fn 对方听不到本机的广播时_编号大的等三秒自己去连() {
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let hello = beacon(&[1; 16], a.port(), Os::Mac, &team().tag(&[1; 16]), "甲");
        probe.send_to(&hello, local(&b)).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(b.connected().is_empty(), "先等编号小的来连");
        std::thread::sleep(Duration::from_millis(2700));
        probe.send_to(&hello, local(&b)).unwrap();
        connected(&rb, 1); connected(&ra, 2);
    }

    #[test]
    fn 校验码不对加不进来_连着错五次歇一会儿() {
        let (a, ra, _) = engine(1, "Mac", vec![], None);
        let (b, rb, _) = engine(2, "PC", vec![], None);
        let failed = |rx: &Receiver<Notice>| wait(rx, |n| match n { Notice::JoinFailed { name, why } => Some((name, why)), _ => None });
        for i in 0..5 {
            a.join(local(&b), "AAAA2222");
            assert_eq!(failed(&ra), ("PC".to_string(), 1), "第 {i} 次");
        }
        // 第六次，码对了也不行：暂时不接受
        a.join(local(&b), &b.group().code);
        assert_eq!(failed(&ra).1, 3);
        assert!(rb.try_iter().all(|n| !matches!(n, Notice::JoinRequest { .. } | Notice::Connected { .. })));
        assert!(a.connected().is_empty() && b.connected().is_empty());
        // 歇够了又可以
        b.shared.lock().unwrap().join_locked = None;
        ask(&a, &b, &rb);
        b.confirm_join([1; 16], true);
        connected(&ra, 2);
    }

    #[test]
    fn 对方不同意就加不进来() {
        let (a, ra, _) = engine(1, "Mac", vec![], None);
        let (b, rb, _) = engine(2, "PC", vec![], None);
        ask(&a, &b, &rb);
        b.confirm_join([1; 16], false);
        assert_eq!(wait(&ra, |n| match n { Notice::JoinFailed { name, why } => Some((name, why)), _ => None }), ("PC".to_string(), 2));
        std::thread::sleep(Duration::from_millis(200));
        assert!(a.connected().is_empty() && b.connected().is_empty());
        assert_ne!(a.group(), b.group());
        assert!(!b.event([1; 16], Event::Leave));
        // 没人申请的时候点“允许”没有用
        b.confirm_join([1; 16], true);
        assert!(b.connected().is_empty());
    }

    #[test]
    fn 不同组的电脑不输对校验码连不上() {
        let (a, ra, _) = engine(1, "Mac", vec![], None);
        let (b, rb, _) = engine(2, "PC", vec![], None);
        // 没填、填短了：不用问对方就知道不对（对方那边不算一次输错）
        for code in ["", "K7M4"] {
            a.join(local(&b), code);
            assert_eq!(wait(&ra, |n| match n { Notice::JoinFailed { why, .. } => Some(why), _ => None }), 1);
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(b.shared.lock().unwrap().join_fails, 0);
        // 带着别的组的记号的广播：只当作“发现了一台”，不会自己去连
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.send_to(&beacon(&[2; 16], b.port(), Os::Windows, &b.group().tag(&[2; 16]), "PC"), local(&a)).unwrap();
        wait(&ra, |n| matches!(n, Notice::Found { .. }).then_some(()));
        std::thread::sleep(Duration::from_millis(300));
        assert!(a.connected().is_empty() && b.connected().is_empty());
        assert!(rb.try_iter().all(|n| !matches!(n, Notice::JoinRequest { .. } | Notice::Connected { .. })));
    }

    #[test]
    fn 换组只带自己_原来组里的电脑留在原组() {
        // 甲、乙一组；丙自己一组。乙去加入丙：乙离开甲，甲还在原来的组里
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        let (c, rc, _) = engine(3, "丙", vec![], None);
        a.join(local(&b), "");
        connected(&ra, 2); connected(&rb, 1);
        ask(&b, &c, &rc);
        c.confirm_join([2; 16], true);
        wait(&rb, |n| match n { Notice::Group { joined, .. } => Some(assert!(joined)), _ => None });
        wait(&ra, |n| match n { Notice::Disconnected { id } if id == [2; 16] => Some(()), _ => None });
        connected(&rb, 3);
        assert_eq!(b.group(), c.group());
        assert_eq!(a.group(), team());
        assert_eq!(b.connected(), vec![[3; 16]]);
        // 甲听到乙的广播：记号已经不是本组的了，只当作发现了一台
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        probe.send_to(&beacon(&[2; 16], b.port(), Os::Windows, &b.group().tag(&[2; 16]), "乙"), local(&a)).unwrap();
        wait(&ra, |n| match n { Notice::Found { id, .. } if id == [2; 16] => Some(()), _ => None });
        assert!(a.connected().is_empty());
    }

    #[test]
    fn 请出一台_这一组换校验码_它拿着旧的进不来() {
        // 甲、乙、丙一组，互相连着。甲把丙请出去
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        let (c, rc, _) = engine(3, "丙", vec![([0; 16], [0; 32])], None);
        a.join(local(&b), ""); connected(&ra, 2); connected(&rb, 1);
        a.join(local(&c), ""); connected(&ra, 3); connected(&rc, 1);
        b.join(local(&c), ""); connected(&rb, 3); connected(&rc, 2);
        a.forget([3; 16]);
        let new = wait(&ra, |n| match n { Notice::Group { code, joined, .. } => { assert!(!joined); Some(code) } _ => None });
        assert_eq!(wait(&rb, |n| match n { Notice::Group { code, joined, .. } => { assert!(!joined); Some(code) } _ => None }), new);
        assert_eq!(a.group(), b.group());
        assert_eq!(a.group().rev, 2);
        assert_ne!(a.group().key, team().key);
        assert_eq!(c.group(), team(), "被请出去的那台没被告知");
        // 丙和甲、乙都断开了；甲、乙还连着
        let end = Instant::now() + Duration::from_secs(5);
        while !c.connected().is_empty() || b.connected() != vec![[1; 16]] || a.connected() != vec![[2; 16]] {
            assert!(Instant::now() < end, "甲 {:?} 乙 {:?} 丙 {:?}", a.connected(), b.connected(), c.connected());
            std::thread::sleep(Duration::from_millis(20));
        }
        // 丙拿着旧的组钥匙再去连乙：不认
        while rc.try_recv().is_ok() {}
        c.join(local(&b), "TEAM2345");
        assert_eq!(wait(&rc, |n| match n { Notice::JoinFailed { why, .. } => Some(why), _ => None }), 1);
        assert!(c.connected().is_empty());
    }

    #[test]
    fn 换一个校验码_连着的组员跟着换() {
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        a.join(local(&b), ""); connected(&ra, 2); connected(&rb, 1);
        a.new_code();
        wait(&rb, |n| match n { Notice::Group { joined, .. } => Some(assert!(!joined)), _ => None });
        assert_eq!(a.group(), b.group());
        assert_eq!(b.group().rev, 2);
        assert_eq!((a.connected().len(), b.connected().len()), (1, 1), "换码不断开");
        // 两台同时换：最后落在同一个上
        a.new_code(); b.new_code();
        let end = Instant::now() + Duration::from_secs(5);
        while a.group() != b.group() { assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(20)) }
        assert_eq!(a.group().rev, 3);
        // 版本号顶到头的不认（不然这一组以后再也换不了）
        { let mut s = b.shared.lock().unwrap();
          let p = s.peers.get_mut(&[1; 16]).unwrap();
          send_tcp(p, &Msg::Regroup { code: "ZZZZ9999".into(), key: [255; 32], rev: u64::MAX, gone: None }); }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(a.group().rev, 3);
        a.new_code();
        let end = Instant::now() + Duration::from_secs(5);
        while b.group().rev != 4 { assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(20)) }
    }

    #[test]
    fn 请出一台的同时别人在换校验码_请出照样算数() {
        let (a, ra, _) = engine(1, "甲", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "乙", vec![([0; 16], [0; 32])], None);
        let (c, rc, _) = engine(3, "丙", vec![([0; 16], [0; 32])], None);
        a.join(local(&b), ""); connected(&ra, 2); connected(&rb, 1);
        a.join(local(&c), ""); connected(&ra, 3); connected(&rc, 1);
        b.join(local(&c), ""); connected(&rb, 3); connected(&rc, 2);
        a.forget([3; 16]); b.new_code();
        let end = Instant::now() + Duration::from_secs(5);
        while a.group() != b.group() || c.group() == a.group() || !c.connected().is_empty() || a.connected() != vec![[2; 16]] {
            assert!(Instant::now() < end, "甲 {:?} 乙 {:?} 丙 {:?}；甲连着 {:?}，丙连着 {:?}", a.group().rev, b.group().rev, c.group().rev, a.connected(), c.connected());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn 屏幕大小变了会告诉对方() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        let (b, rb, _) = engine(2, "PC", vec![([1; 16], ida.public)], Some(idb));
        b.join(local(&a), "");
        wait(&ra, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        b.set_size(3840, 2160);
        assert_eq!(wait(&ra, |n| match n { Notice::Resized { width, height, .. } => Some((width, height)), _ => None }), (3840, 2160));
        assert!(!a.send_clip_text([2; 16], &"x".repeat(MAX_CLIP + 1), ClipTag::default()), "太长的剪贴板不发");
    }

    #[test]
    fn 剪贴板里的图片带过去_两边都不提示() {
        let (a, ra, _) = engine(1, "Mac", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(2, "PC", vec![([0; 16], [0; 32])], None);
        b.join(local(&a), ""); connected(&ra, 2); connected(&rb, 1);
        let src = std::env::temp_dir().join(format!("yiwei-test-clip-{}.png", std::process::id()));
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        assert!(a.send_clip_image([2; 16], src.clone(), ClipTag { back: true, no: 5 }));
        let path = wait(&rb, |n| match n { Notice::ClipImage { id, path, tag } => { assert_eq!(id, [1; 16]); assert_eq!(tag, ClipTag { back: true, no: 5 }, "记号原样带到"); Some(path) }
                                           Notice::RecvBegin { .. } | Notice::RecvDone { .. } | Notice::RecvProgress { .. } => panic!("剪贴板的图片不该当成收文件来提示"), _ => None });
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let _ = std::fs::remove_file(&path);
        let end = Instant::now() + Duration::from_secs(3);
        while src.exists() { assert!(Instant::now() < end, "发完要把外壳存的那个文件删掉"); std::thread::sleep(Duration::from_millis(20)) }
        assert!(ra.try_iter().all(|n| !matches!(n, Notice::SendProgress { .. } | Notice::SendDone { .. })));
        // 没连着的、文件不存在的：不发，也不留垃圾
        assert!(!a.send_clip_image([9; 16], src.clone(), ClipTag::default()));
    }

    #[test]
    fn 复制的文件跟着指针带过去_原件不动_太大的不带() {
        let (a, ra, _) = engine(41, "Mac", vec![([0; 16], [0; 32])], None);
        let (b, rb, _) = engine(42, "PC", vec![([0; 16], [0; 32])], None);
        b.join(local(&a), ""); connected(&ra, 42); connected(&rb, 41);
        let src = std::env::temp_dir().join(format!("yiwei-test-clipfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("资料/里面")).unwrap();
        std::fs::write(src.join("资料/里面/一.txt"), "你好").unwrap();
        std::fs::write(src.join("资料/二.bin"), vec![7u8; 200_000]).unwrap();
        std::fs::write(src.join("单个.txt"), "一个文件").unwrap();
        let tag = ClipTag { back: false, no: 3 };
        assert_eq!(a.send_clip_files([42; 16], vec![src.join("资料"), src.join("单个.txt")], tag), Some(true));
        let got = wait(&rb, |n| match n { Notice::ClipFiles { id, paths, tag: t } => { assert_eq!((id, t), ([41; 16], tag)); Some(paths) }
                                          Notice::RecvBegin { .. } | Notice::RecvDone { .. } | Notice::RecvProgress { .. } => panic!("带剪贴板的文件不该当成收文件来提示"), _ => None });
        assert_eq!(got.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(), vec!["资料", "单个.txt"], "交给剪贴板的是最上层的那几个");
        assert_eq!(std::fs::read_to_string(got[0].join("里面/一.txt")).unwrap(), "你好");
        assert_eq!(std::fs::read(got[0].join("二.bin")).unwrap().len(), 200_000);
        assert!(!got[0].starts_with(&src) && src.join("资料/二.bin").exists() && src.join("单个.txt").exists(), "原件一个都不能动");
        assert!(ra.try_iter().all(|n| !matches!(n, Notice::SendProgress { .. } | Notice::SendDone { .. })));
        // 同一台电脑再带一批来：上一批清掉
        assert_eq!(a.send_clip_files([42; 16], vec![src.join("单个.txt")], ClipTag { back: false, no: 4 }), Some(true));
        let again = wait(&rb, |n| match n { Notice::ClipFiles { paths, .. } => Some(paths), _ => None });
        assert!(again[0].exists() && !got[0].exists());
        // 超过 100MB（用一个只占名义大小的文件）、没连着的：不带
        let big = std::fs::File::create(src.join("大.bin")).unwrap(); big.set_len(MAX_CLIP_FILES + 1).unwrap(); drop(big);
        assert_eq!(a.send_clip_files([42; 16], vec![src.join("大.bin")], tag), Some(false));
        assert_eq!(a.send_clip_files([9; 16], vec![src.join("单个.txt")], tag), None);
        // 空文件夹、不在了的：没什么可带的，不算“太大”
        std::fs::create_dir_all(src.join("空的")).unwrap();
        assert_eq!(a.send_clip_files([42; 16], vec![src.join("空的"), src.join("没有这个")], tag), None);
        // 晚到的更早的一批：收是收下了，但不能把新的那批清掉
        assert_eq!(a.send_clip_files([42; 16], vec![src.join("单个.txt")], ClipTag { back: false, no: 2 }), Some(true));
        let late = wait(&rb, |n| match n { Notice::ClipFiles { paths, .. } => Some(paths), _ => None });
        assert!(late[0].exists() && again[0].exists());
        if let Some(d) = late[0].parent() { let _ = std::fs::remove_dir_all(d); }
        let _ = std::fs::remove_dir_all(&src);
        if let Some(d) = again[0].parent() { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn 发到一半取消_对方删掉没收完的() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        let (b, rb, _) = engine(2, "PC", vec![([1; 16], ida.public)], Some(idb));
        b.join(local(&a), "");
        wait(&ra, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        let base = std::env::temp_dir().join(format!("yiwei-net-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base); std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("大.bin"), vec![1u8; 200_000_000]).unwrap();
        b.set_recv_dir(base.join("收"));
        let x = a.send_files([2; 16], vec![base.join("大.bin")]).unwrap();
        wait(&rb, |n| matches!(n, Notice::RecvProgress { .. }).then_some(()));
        a.cancel_send(x);
        let err = wait(&ra, |n| match n { Notice::SendDone { ok: false, err, .. } => Some(err), Notice::SendDone { ok: true, .. } => panic!("应该取消了"), _ => None });
        assert_eq!(err, "取消了");
        wait(&rb, |n| match n { Notice::RecvDone { ok, .. } => { assert!(!ok); Some(()) } _ => None });
        assert_eq!(std::fs::read_dir(base.join("收")).unwrap().count(), 0, "没收完的不留");
        assert!(a.connected().len() == 1, "取消不影响连接");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 收的一方取消_发的一方知道() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        let (b, rb, _) = engine(2, "PC", vec![([1; 16], ida.public)], Some(idb));
        b.join(local(&a), "");
        wait(&ra, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        let base = std::env::temp_dir().join(format!("yiwei-net-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base); std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("大.bin"), vec![1u8; 200_000_000]).unwrap();
        b.set_recv_dir(base.join("收"));
        a.send_files([2; 16], vec![base.join("大.bin")]).unwrap();
        let x = wait(&rb, |n| match n { Notice::RecvBegin { xfer, .. } => Some(xfer), _ => None });
        b.cancel_recv([1; 16], x);
        let err = wait(&ra, |n| match n { Notice::SendDone { ok: false, err, .. } => Some(err), Notice::SendDone { ok: true, .. } => panic!("应该被对方取消"), _ => None });
        assert_eq!(err, "对方：取消了");
        wait(&rb, |n| match n { Notice::RecvDone { ok, .. } => { assert!(!ok); Some(()) } _ => None });
        assert_eq!(std::fs::read_dir(base.join("收")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 来不及照做时_排着队的鼠标移动并成一次() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        // 乙这边照做一次要 20 毫秒
        let (tx, rb) = channel(); let tx = Mutex::new(tx);
        let cfg = Config { id: [2; 16], name: "PC".into(), os: Os::Windows, width: 1920, height: 1080, identity: idb, group: { let _ = &ida; team() }, port: 0, discover: false };
        let b = Engine::start(cfg, move |n| { if matches!(n, Notice::Input { .. }) { std::thread::sleep(Duration::from_millis(20)) } let _ = tx.lock().unwrap().send(n); }).unwrap();
        b.join(local(&a), "");
        wait(&ra, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        // 甲半秒里发 200 次移动，中间夹一次按键
        for i in 0..200 {
            a.motion([2; 16], 1, 2);
            if i == 100 { assert!(a.event([2; 16], Event::Key { usage: 4, down: true })) }
            std::thread::sleep(Duration::from_micros(2500));
        }
        let (mut moves, mut walked, mut keys, mut at_key) = (0, (0, 0), 0, (0, 0));
        let end = Instant::now() + Duration::from_secs(5);
        while walked != (200, 400) {
            assert!(Instant::now() < end, "一共走的一定要一样多，现在才 {walked:?}");
            for act in wait(&rb, |n| match n { Notice::Input { actions, .. } => Some(actions), _ => None }) {
                match act { Action::Motion { dx, dy } => { moves += 1; walked = (walked.0 + dx, walked.1 + dy) } Action::Event(Event::Key { .. }) => { keys += 1; at_key = walked } _ => {} }
            }
        }
        assert_eq!(keys, 1, "按键一个不少");
        assert_eq!(at_key, (101, 202), "按键的时候鼠标正好走到它发生时的地方");
        assert!(moves < 80, "200 次移动应该并成少得多的几次，实际照做了 {moves} 次");
    }

    #[test]
    fn 版本不一样会告诉外壳() {
        let (a, ra, _) = engine(1, "Mac", vec![], None);
        let me = Identity::generate().unwrap();
        let mut s = TcpStream::connect(local(&a)).unwrap();
        let (mut h, m1) = Handshake::start(&me).unwrap();
        write_hs(&mut s, &m1).unwrap();
        let m3 = h.hear(&read_hs(&mut s).unwrap()).unwrap().unwrap();
        write_hs(&mut s, &m3).unwrap();
        let mut session = h.finish().unwrap();
        let hello = Msg::Hello { protocol: PROTOCOL + 1, id: [9; 16], os: Os::Windows, name: "新版的电脑".into(), width: 1, height: 1, port: 1, proof: [0; 32] };
        s.write_all(&session.seal(&hello.encode()).unwrap()).unwrap();
        let (name, theirs) = wait(&ra, |n| match n { Notice::Mismatch { name, theirs } => Some((name, theirs)), _ => None });
        assert_eq!((name.as_str(), theirs), ("新版的电脑", PROTOCOL as u32 + 1));
        assert!(a.connected().is_empty());
    }

    #[test]
    fn 没事的时候补发线程睡着_有事马上醒() {
        let (ida, idb) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (a, ra, _) = engine(1, "Mac", vec![([2; 16], idb.public)], Some(ida.clone()));
        let (b, rb, _) = engine(2, "PC", vec![([1; 16], ida.public)], Some(idb));
        b.join(local(&a), "");
        wait(&ra, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        wait(&rb, |n| matches!(n, Notice::Connected { .. }).then_some(()));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*a.pace.level.lock().unwrap(), 0, "连着但没在控制：不忙");
        // 甲控制乙：按键马上到；确认完以后只剩“报平安”的档
        assert!(a.event([2; 16], Event::Key { usage: 4, down: true }));
        a.hold([2; 16], true); a.motion([2; 16], 10, 10);
        wait(&rb, |n| matches!(n, Notice::Input { .. }).then_some(()));
        let end = Instant::now() + Duration::from_secs(2);
        while *a.pace.level.lock().unwrap() != 1 { assert!(Instant::now() < end, "确认完应该降到 1"); std::thread::sleep(Duration::from_millis(5)) }
        a.hold([2; 16], false);
        let end = Instant::now() + Duration::from_secs(2);
        while *a.pace.level.lock().unwrap() != 0 { assert!(Instant::now() < end, "不控制了应该降到 0"); std::thread::sleep(Duration::from_millis(5)) }
        assert_eq!(*b.pace.level.lock().unwrap(), 0, "被控的一方一直不忙");
    }
}
