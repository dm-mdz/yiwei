//! 外壳的键鼠监听回调和核心之间的那一层。
//!
//! 系统是一个事件一个事件、同步地调监听回调的：回调里一慢，整台电脑的鼠标键盘就跟着卡。
//! 所以回调里只做三件事：读几个原子量决定“这次输入本机还要不要”，把事件放进一个定长队列，叫醒后台线程。
//! 不加密、不发网络、不写日志、不等任何锁。判断穿屏、加密、发包都在后台线程里做（见 ffi.rs 的 `work`）。
//!
//! 本机鼠标平时的移动绝大多数根本不用报：只有贴着屏幕边缘（可能要穿屏）、或者别的电脑的鼠标正在本机上
//! （本机鼠标把指针挪到哪了，对方要接着从那里走）的时候才放进队列。平时用电脑，后台线程一直睡着。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::OnceLock;
use std::thread::Thread;
use std::time::Instant;

/// 外壳报来的一次本机输入。taken：回调当时有没有把它从本机拿走（拿走的发给正在控制的那台电脑）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ev {
    LocalMove { x: f64, y: f64, dx: f64, dy: f64 },
    RemoteMove { dx: f64, dy: f64 },
    Key { usage: u16, down: bool, taken: bool },
    Button { button: u8, down: bool, taken: bool },
    Wheel { dx: i32, dy: i32, taken: bool },
}

/// 队列能放这么多个事件。后台线程处理一个事件只要几微秒，放满说明它卡住了：新的事件丢掉并记数
const N: usize = 1024;

/// 定长队列：放的一头可以有多个线程（用一个标志排队，标志只在放一个事件的那几十纳秒里占着），取的一头只有后台线程
struct Ring { buf: UnsafeCell<[Ev; N]>, head: AtomicUsize, tail: AtomicUsize, writing: AtomicBool }
unsafe impl Sync for Ring {}
impl Ring {
    fn new() -> Ring { Ring { buf: UnsafeCell::new([Ev::RemoteMove { dx: 0.0, dy: 0.0 }; N]), head: AtomicUsize::new(0), tail: AtomicUsize::new(0), writing: AtomicBool::new(false) } }
    fn push(&self, ev: Ev) -> bool {
        while self.writing.compare_exchange_weak(false, true, SeqCst, SeqCst).is_err() { std::hint::spin_loop() }
        let (t, h) = (self.tail.load(SeqCst), self.head.load(SeqCst));
        let ok = t.wrapping_sub(h) < N;
        if ok {
            // 取的一头只读 head 到 tail 之间的格子，这一格在 tail 之外，没人同时碰它
            unsafe { (self.buf.get() as *mut Ev).add(t % N).write(ev) }
            self.tail.store(t.wrapping_add(1), SeqCst);
        }
        self.writing.store(false, SeqCst);
        ok
    }
    fn pop(&self) -> Option<Ev> {
        let h = self.head.load(SeqCst);
        if h == self.tail.load(SeqCst) { return None }
        let ev = unsafe { (self.buf.get() as *const Ev).add(h % N).read() };
        self.head.store(h.wrapping_add(1), SeqCst);
        Some(ev)
    }
}

pub struct Front {
    ring: Ring,
    /// 鼠标在别的电脑上（后台线程处理到“穿走 / 回来”时改）
    remote: AtomicBool,
    /// 别的电脑的鼠标正在本机上
    bossed: AtomicBool,
    size: (AtomicU32, AtomicU32),
    /// 给了本机、还没抬起的键（每个键位一位）：它们的抬起也要给本机，不然本机这个键会一直按着
    keys: [AtomicU64; 4],
    /// 同上，鼠标键
    buttons: AtomicU32,
    /// 这台电脑的大写锁定是不是开着（外壳贴边时报上来）
    caps: AtomicBool,
    /// 修饰键的“按下”因为本机记着它还按着而没发出去的次数（排查“Shift 没带过去”用）
    swallowed: AtomicU32,
    home: AtomicBool,
    stop: AtomicBool,
    dropped: AtomicU32,
    worker: OnceLock<Thread>,
    born: Instant,
    /// 队列里最早那个还没处理的事件是什么时候放进来的（从 born 起的微秒，0 表示没有）：用来量从回调到处理隔了多久
    since: AtomicU64,
}

impl Front {
    pub fn new(width: u32, height: u32) -> Front {
        Front { ring: Ring::new(), remote: AtomicBool::new(false), bossed: AtomicBool::new(false), size: (AtomicU32::new(width), AtomicU32::new(height)),
                keys: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)], buttons: AtomicU32::new(0),
                caps: AtomicBool::new(false), swallowed: AtomicU32::new(0), home: AtomicBool::new(false), stop: AtomicBool::new(false), dropped: AtomicU32::new(0), worker: OnceLock::new(),
                born: Instant::now(), since: AtomicU64::new(0) }
    }
    fn push(&self, ev: Ev) {
        if !self.ring.push(ev) { self.dropped.fetch_add(1, SeqCst); }
        if self.since.load(SeqCst) == 0 { self.since.store((self.born.elapsed().as_micros() as u64).max(1), SeqCst) }
        self.wake();
    }
    /// 叫醒后台线程
    pub fn wake(&self) { if let Some(t) = self.worker.get() { t.unpark() } }

    // —— 监听回调里调的 ——

    pub fn remote(&self) -> bool { self.remote.load(SeqCst) }
    /// 鼠标在本机时的移动
    pub fn local_move(&self, x: f64, y: f64, dx: f64, dy: f64) {
        if self.remote.load(SeqCst) { return }
        let (w, h) = (self.size.0.load(SeqCst) as f64, self.size.1.load(SeqCst) as f64);
        let at_edge = x <= 0.0 || y <= 0.0 || x >= w - 1.0 || y >= h - 1.0;
        if at_edge || self.bossed.load(SeqCst) { self.push(Ev::LocalMove { x, y, dx, dy }) }
    }
    /// 鼠标在别的电脑上时，本机鼠标的移动量
    pub fn remote_move(&self, dx: f64, dy: f64) { if self.remote.load(SeqCst) { self.push(Ev::RemoteMove { dx, dy }) } }
    /// 键盘。返回 true：这个键拿走了，本机不要再处理
    pub fn key(&self, usage: u16, down: bool) -> bool {
        let remote = self.remote.load(SeqCst);
        let taken = match self.keys.get(usage as usize >> 6) {
            None => remote,                                                      // 表外的键位：不记，鼠标在哪就给哪
            Some(word) => {
                let bit = 1u64 << (usage & 63);
                if !down { let local = word.fetch_and(!bit, SeqCst) & bit != 0; remote && !local }
                else if !remote { word.fetch_or(bit, SeqCst); false }
                else if word.load(SeqCst) & bit != 0 {                           // 穿走之前就按着的键自动重复：哪边都不给
                    if usage >= 0xE0 && usage <= 0xE7 { self.swallowed.fetch_add(1, SeqCst); self.wake() }
                    return true;
                }
                else { true }
            }
        };
        self.push(Ev::Key { usage, down, taken });
        taken
    }
    /// 鼠标键。返回值同上
    pub fn button(&self, button: u8, down: bool) -> bool {
        let remote = self.remote.load(SeqCst);
        let bit = 1u32 << (button.min(31) as u32);
        let taken = if !down { let local = self.buttons.fetch_and(!bit, SeqCst) & bit != 0; remote && !local }
                    else if !remote { self.buttons.fetch_or(bit, SeqCst); false }
                    else { true };
        self.push(Ev::Button { button, down, taken });
        taken
    }
    /// 滚轮。返回值同上
    pub fn wheel(&self, dx: i32, dy: i32) -> bool {
        let taken = self.remote.load(SeqCst);
        if taken || self.bossed.load(SeqCst) { self.push(Ev::Wheel { dx, dy, taken }) }
        taken
    }
    /// 贴边要穿屏之前，外壳按系统说的“现在真按着哪些修饰键”对一次（mask 第 0–7 位对应键位 0xE0–0xE7）。
    /// 这里记着按着、系统说没按着的，是抬起那一下没监听到（锁屏、密码框、管理员窗口）：补一次抬起，
    /// 不然每次穿屏都会带着这个修饰键过去，对方一进来就替它按住
    pub fn sync_mods(&self, mask: u32) {
        self.caps.store(mask & 0x100 != 0, SeqCst);
        let stale = (self.keys[3].load(SeqCst) >> 32) as u32 & 0xFF & !mask;
        for i in 0..8u16 { if stale & (1 << i) != 0 { self.key(0xE0 + i, false); } }
    }
    /// 紧急拉回：请后台线程马上把鼠标拉回本机
    pub fn request_home(&self) { self.home.store(true, SeqCst); self.wake() }

    // —— 后台线程和核心别处用的 ——

    pub fn caps(&self) -> bool { self.caps.load(SeqCst) }
    pub fn take_swallowed(&self) -> u32 { self.swallowed.swap(0, SeqCst) }
    pub fn set_remote(&self, v: bool) { self.remote.store(v, SeqCst) }
    pub fn set_bossed(&self, v: bool) { self.bossed.store(v, SeqCst) }
    pub fn set_size(&self, width: u32, height: u32) { self.size.0.store(width, SeqCst); self.size.1.store(height, SeqCst) }
    pub fn set_worker(&self, t: Thread) { let _ = self.worker.set(t); }
    pub fn request_stop(&self) { self.stop.store(true, SeqCst); self.wake() }
    pub fn stopping(&self) -> bool { self.stop.load(SeqCst) }
    pub fn take_home(&self) -> bool { self.home.swap(false, SeqCst) }
    pub fn take_dropped(&self) -> u32 { self.dropped.swap(0, SeqCst) }
    /// 刚取出来的这批事件里，最早的那个等了多久（微秒）。取完队列之后调
    pub fn take_wait_us(&self) -> u64 {
        let t = self.since.swap(0, SeqCst);
        if t == 0 { 0 } else { (self.born.elapsed().as_micros() as u64).saturating_sub(t) }
    }
    /// 把队列里的事件都取出来，接在 batch 后面。连着的几次移动并成一次（位置用最后的，移动量加起来）
    pub fn drain(&self, batch: &mut Vec<Ev>) {
        while let Some(ev) = self.ring.pop() {
            match (batch.last_mut(), ev) {
                (Some(Ev::RemoteMove { dx, dy }), Ev::RemoteMove { dx: a, dy: b }) => { *dx += a; *dy += b }
                (Some(Ev::LocalMove { x, y, dx, dy }), Ev::LocalMove { x: nx, y: ny, dx: a, dy: b }) => { *x = nx; *y = ny; *dx += a; *dy += b }
                _ => batch.push(ev),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(f: &Front) -> Vec<Ev> { let mut v = Vec::new(); f.drain(&mut v); v }

    #[test]
    fn 平时的鼠标移动不用报_贴边或被控时才报() {
        let f = Front::new(1920, 1080);
        f.local_move(900.0, 500.0, 3.0, 0.0);
        assert_eq!(all(&f), vec![]);
        f.local_move(1919.0, 500.0, 3.0, 0.0);
        f.local_move(0.0, 500.0, -3.0, 0.0);
        assert_eq!(all(&f).len(), 1, "连着的两次并成一次");
        f.set_bossed(true);
        f.local_move(900.0, 500.0, 3.0, 0.0);
        assert_eq!(all(&f), vec![Ev::LocalMove { x: 900.0, y: 500.0, dx: 3.0, dy: 0.0 }]);
        f.set_size(800, 600);
        f.set_bossed(false);
        f.local_move(799.0, 10.0, 1.0, 0.0);
        assert_eq!(all(&f).len(), 1, "屏幕变小以后按新的边算");
        f.remote_move(5.0, 5.0);
        assert_eq!(all(&f), vec![], "鼠标在本机时没有“在对方屏幕上的移动”");
    }

    #[test]
    fn 连着的移动并成一次_中间隔着别的就不并() {
        let f = Front::new(1920, 1080);
        f.set_remote(true);
        f.remote_move(1.0, 2.0); f.remote_move(3.0, -1.0);
        f.button(1, true);
        f.remote_move(5.0, 5.0);
        assert_eq!(all(&f), vec![Ev::RemoteMove { dx: 4.0, dy: 1.0 }, Ev::Button { button: 1, down: true, taken: true }, Ev::RemoteMove { dx: 5.0, dy: 5.0 }]);
        f.set_remote(false);
        f.local_move(1919.0, 10.0, 2.0, 0.0); f.local_move(1919.0, 12.0, 1.0, 2.0);
        assert_eq!(all(&f), vec![Ev::LocalMove { x: 1919.0, y: 12.0, dx: 3.0, dy: 2.0 }]);
    }

    #[test]
    fn 在本机按下的键_抬起也给本机() {
        let f = Front::new(1920, 1080);
        assert!(!f.key(0xE1, true));                         // 本机按住 Shift
        assert!(!f.key(0x04, true));                         // 本机还按着 A
        f.set_remote(true);                                  // 鼠标穿走了
        assert!(f.key(0x04, true));                          // A 的自动重复：本机不要，也不发
        assert!(f.key(0x05, true));                          // 在那边按的 B：拿走
        assert!(!f.key(0xE1, false));                        // Shift 抬起：给本机
        assert!(!f.key(0x04, false));                        // A 抬起：给本机
        assert!(f.key(0x05, false));                         // B 抬起：拿走
        assert!(f.key(0x06, true));                          // 在那边按下 C，没抬起鼠标就回来了
        f.set_remote(false);
        assert!(!f.key(0x06, false));                        // 抬起给本机（本机没见过它按下，多一次抬起无妨）
        assert_eq!(all(&f), vec![
            Ev::Key { usage: 0xE1, down: true, taken: false }, Ev::Key { usage: 0x04, down: true, taken: false },
            Ev::Key { usage: 0x05, down: true, taken: true },
            Ev::Key { usage: 0xE1, down: false, taken: false }, Ev::Key { usage: 0x04, down: false, taken: false },
            Ev::Key { usage: 0x05, down: false, taken: true }, Ev::Key { usage: 0x06, down: true, taken: true },
            Ev::Key { usage: 0x06, down: false, taken: false },
        ]);
        f.set_remote(true);
        assert!(f.key(0x300, true) && f.key(0x300, false), "表外的键位照鼠标在哪来分");
    }

    #[test]
    fn 没监听到抬起的修饰键_贴边时按系统说的补上() {
        let f = Front::new(1920, 1080);
        f.key(0xE0, true); f.key(0xE1, true); f.key(0xE7, true); f.key(0x04, true);      // 左 Ctrl、左 Shift、右 Win、A
        all(&f);
        f.sync_mods(0b0000_0010);                                                        // 系统说只有左 Shift 还按着
        assert_eq!(all(&f), vec![Ev::Key { usage: 0xE0, down: false, taken: false }, Ev::Key { usage: 0xE7, down: false, taken: false }]);
        f.sync_mods(0b0000_0010);
        assert_eq!(all(&f), vec![], "补过就不再补；普通键不管");
        f.sync_mods(0xFF);
        assert_eq!(all(&f), vec![], "系统说按着、这里没记的（别的电脑注入的）不动");
        f.sync_mods(0);
        assert_eq!(all(&f), vec![Ev::Key { usage: 0xE1, down: false, taken: false }]);
    }

    #[test]
    fn 在本机按下的鼠标键_抬起也给本机() {
        let f = Front::new(1920, 1080);
        assert!(!f.button(1, true));
        f.set_remote(true);
        assert!(f.button(2, true));
        assert!(!f.button(1, false));
        assert!(f.button(2, false));
        assert!(f.wheel(0, -120));
        f.set_remote(false);
        assert!(!f.wheel(0, -120));
        assert_eq!(all(&f).len(), 5, "鼠标在本机、又没被控制时，滚轮不用报");
        f.set_bossed(true);
        assert!(!f.wheel(0, -120));
        assert_eq!(all(&f), vec![Ev::Wheel { dx: 0, dy: -120, taken: false }]);
    }

    #[test]
    fn 队列放满就丢并记数_取走以后接着用() {
        let f = Front::new(1920, 1080);
        f.set_remote(true);
        for i in 0..N + 10 { f.button((i % 5) as u8, true); }
        assert_eq!(f.take_dropped(), 10);
        assert_eq!(f.take_dropped(), 0);
        assert_eq!(all(&f).len(), N);
        for _ in 0..3 { for i in 0..N { f.wheel(i as i32, 0); } let v = all(&f); assert_eq!(v.len(), N); assert_eq!(v[N - 1], Ev::Wheel { dx: N as i32 - 1, dy: 0, taken: true }); }
        assert_eq!(f.take_dropped(), 0);
        assert!(!f.take_home());
        f.request_home();
        assert!(f.take_home() && !f.take_home());
    }

    #[test]
    fn 几个线程同时放也不乱() {
        let f = std::sync::Arc::new(Front::new(1920, 1080));
        f.set_remote(true);
        let hs: Vec<_> = (0..4).map(|t| { let f = f.clone(); std::thread::spawn(move || for i in 0..200 { f.wheel(t, i); }) }).collect();
        for h in hs { h.join().unwrap() }
        let v = all(&f);
        assert_eq!(v.len(), 800);
        for t in 0..4 {
            let mine: Vec<i32> = v.iter().filter_map(|e| match e { Ev::Wheel { dx, dy, .. } if *dx == t => Some(*dy), _ => None }).collect();
            assert_eq!(mine, (0..200).collect::<Vec<_>>(), "每个线程自己放的先后不变");
        }
    }
}
