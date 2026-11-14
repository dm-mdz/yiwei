//! 谁在控制谁：鼠标到了屏幕边缘、那一侧摆着另一台电脑，就穿过去；在那边走到靠回来的那条边，就回来。
//!
//! 规矩（2026-11-13 改成照 Lan Mouse 的对称做法，发送照 Moonlight）：
//!   · 没有主从，每台电脑都可以把鼠标穿到别的电脑上。谁最后穿过边，谁控制：
//!     收到别的电脑“我进来了”，自己的鼠标要是正在别处，就先放开回本机。
//!   · 鼠标发的是移动量，被控的那台在自己指针现在的位置上加。所以被控那台自己的鼠标也照常能用——
//!     两边的鼠标动的是同一个指针，像一台电脑插了两个鼠标，不用判断谁优先，也就没有“谁把谁赶走”。
//!   · 鼠标什么时候回去，由被控的那台看：指针真走到了它屏幕的边，它就告诉控制方“到这条边了”；
//!     回本机、接着穿到下一台、还是原地不动，由控制方按摆法定。控制方不用知道对方屏幕多大、指针在哪。
//!   · 一台电脑同一时刻要么自己的鼠标在别处，要么有一台别的电脑的鼠标在它这里（下面叫“客人”），不会两样同时：
//!     自己的鼠标要穿出去时，先请客人回去；每次穿屏带一个序号，“回去吧”“到边了”要对上序号才算，晚到的旧话不理。
//!   · 两台屏幕大小不一样时，按比例对过去（从这边的中间出去，就从那边的中间进来）。
//!   · 按着鼠标键拖东西的时候不穿屏，免得拖到一半跑到另一台电脑上。
//!
//! 这里不碰网络也不碰系统：发消息通过 `Link`，要外壳做的事（把本机鼠标藏起来 / 放回来 / 照做对方的输入）通过返回的 `Cmd`。
//! 坐标一律以屏幕左上角为原点，往右、往下为正。

use crate::input::{Action, Event};
use crate::keys::{modifier_bit, ASK_TAKE_BACK, MOD_CAPS};
use crate::layout::{from_permille, to_permille, Layout};
use crate::wire::{DeviceId, Side};
use std::collections::{HashMap, HashSet};

/// 往别的电脑发输入的通道（真实实现是 `net::Wire`，测试里是假的）
pub trait Link {
    fn event(&self, id: DeviceId, ev: Event) -> bool;
    /// 鼠标又移动了这么多
    fn motion(&self, id: DeviceId, dx: i32, dy: i32);
    fn wheel(&self, id: DeviceId, dx: i32, dy: i32);
    /// 开始 / 不再控制这台电脑
    fn hold(&self, id: DeviceId, on: bool);
}

impl Link for crate::net::Wire {
    fn event(&self, id: DeviceId, ev: Event) -> bool { crate::net::Wire::event(self, id, ev) }
    fn motion(&self, id: DeviceId, dx: i32, dy: i32) { crate::net::Wire::motion(self, id, dx, dy) }
    fn wheel(&self, id: DeviceId, dx: i32, dy: i32) { crate::net::Wire::wheel(self, id, dx, dy) }
    fn hold(&self, id: DeviceId, on: bool) { crate::net::Wire::hold(self, id, on) }
}

/// 要外壳做的事
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// 鼠标穿到这台电脑上了：把本机鼠标藏起来停住，之后的键鼠输入不再给本机，改为报给这里
    Grab { peer: DeviceId },
    /// 鼠标回来了：恢复本机键鼠，把鼠标放到这个位置
    Release { x: i32, y: i32 },
    /// 别的电脑的输入，照做（kind 和后面四个数的意思见 yiwei_core.h 的 ywk_input_fn）
    Input { kind: i32, a: i32, b: i32, c: i32, d: i32 },
}
/// 对方的鼠标走了：how 0 正常走了、1 被请回去了（别的电脑接手，或者本机的鼠标自己穿出去了）、2 对方掉线了
fn left(how: i32) -> Cmd { Cmd::Input { kind: 6, a: how, b: 0, c: 0, d: 0 } }

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode { Local, Remote { peer: DeviceId, epoch: u32 } }

/// 鼠标正在本机上的那台电脑：它那次穿屏的序号、指针现在在哪、四条边各自上次告诉它“到边了”是什么时候、上次让外壳把指针放在哪
struct Guest { id: DeviceId, epoch: u32, x: f64, y: f64, told: [Option<u64>; 4], shown: (i32, i32), ask_back: bool }

/// 客人的鼠标顶着同一条边不动时，隔这么久（毫秒）才再告诉它一次“到边了”
const EDGE_REPEAT_MS: u64 = 100;

pub struct Controller {
    me: DeviceId,
    size: (f64, f64),
    peers: HashMap<DeviceId, (f64, f64)>,
    layout: Layout,
    mode: Mode,
    local: (f64, f64),
    buttons: u32,
    /// 按着的修饰键（左右分开记，松开一边不影响另一边）
    held_mods: HashSet<u16>,
    /// 我每把鼠标穿出去一次加一
    epoch: u32,
    guest: Option<Guest>,
    /// 还没发出去的不足一个像素的移动量
    carry: (f64, f64),
    /// 本机的大写锁定是不是开着（穿屏时带过去）
    caps: bool,
    /// “鼠标被控切回”开着：本机去控制别的电脑时，告诉对方“你自己的鼠标一动就请我回去”（对方记在客人的 ask_back 里）。
    /// 只管本机当控制方的时候；本机被控时请不请客人回去，看的是客人那头开没开
    take_back: bool,
    /// 现在的时间（毫秒，只用来算间隔）
    now: u64,
    /// 客人在的时候本机的鼠标最近这一阵挪了多少：从什么时候开始算的、合起来多少像素
    nudge: (u64, f64),
}
/// 本机的鼠标在这么长时间（毫秒）里合起来挪了这么多像素，才算这台的人真在用鼠标（轻轻碰一下不算）
const NUDGE_MS: u64 = 400;
const NUDGE_PX: f64 = 30.0;

/// 从某条边进入一块屏幕时落在哪里：along 是沿着那条边的位置
fn entry(side: Side, along: f64, size: (f64, f64)) -> (f64, f64) {
    match side {
        Side::Left => (1.0, along),
        Side::Right => (size.0 - 2.0, along),
        Side::Top => (along, 1.0),
        Side::Bottom => (along, size.1 - 2.0),
    }
}
fn edge_length(side: Side, size: (f64, f64)) -> f64 { match side { Side::Left | Side::Right => size.1, _ => size.0 } }
fn short(id: &DeviceId) -> String { id[..4].iter().map(|b| format!("{b:02x}")).collect() }

impl Controller {
    pub fn new(me: DeviceId, width: u32, height: u32) -> Controller {
        Controller { me, size: (width as f64, height as f64), peers: HashMap::new(), layout: Layout::default(), mode: Mode::Local, local: (0.0, 0.0),
                     buttons: 0, held_mods: HashSet::new(), epoch: 0, guest: None, carry: (0.0, 0.0), caps: false, take_back: false, now: 0, nudge: (0, 0.0) }
    }
    pub fn set_layout(&mut self, layout: Layout) { self.layout = layout }
    pub fn set_size(&mut self, width: u32, height: u32) { self.size = (width as f64, height as f64) }
    pub fn set_caps(&mut self, on: bool) { self.caps = on }
    pub fn set_take_back(&mut self, on: bool) {
        if self.take_back != on { crate::ylog!("“鼠标被控切回”{}", if on { "打开了" } else { "关掉了" }) }
        self.take_back = on;
    }
    pub fn set_now(&mut self, ms: u64) { self.now = ms }
    pub fn peer_connected(&mut self, id: DeviceId, width: u32, height: u32) { self.peers.insert(id, (width.max(4) as f64, height.max(4) as f64)); }
    /// 正在控制的那台电脑（鼠标在本机时是 None）
    pub fn remote(&self) -> Option<DeviceId> { match self.mode { Mode::Remote { peer, .. } => Some(peer), Mode::Local => None } }
    /// 是不是有别的电脑的鼠标正在本机上
    pub fn hosting(&self) -> bool { self.guest.is_some() }

    fn home(&mut self) -> Cmd { self.mode = Mode::Local; self.carry = (0.0, 0.0); Cmd::Release { x: self.local.0.round() as i32, y: self.local.1.round() as i32 } }
    /// 被对方请回来的：落点离边远一点，不然手上稍微一蹭又穿过去
    fn pull_in(&mut self) {
        let (w, h) = self.size;
        self.local = (self.local.0.clamp(40.0, (w - 41.0).max(40.0)), self.local.1.clamp(40.0, (h - 41.0).max(40.0)));
    }

    /// 一台电脑断开了。鼠标正在它上面就回到本机；它的鼠标正在本机上就替它松开按着的键
    pub fn peer_disconnected(&mut self, id: DeviceId) -> Vec<Cmd> {
        let mut out = Vec::new();
        self.peers.remove(&id);
        if self.guest.as_ref().map_or(false, |g| g.id == id) { self.guest = None; out.push(left(2)) }
        if self.remote() == Some(id) { out.push(self.home()) }
        out
    }

    /// 鼠标从 side 这条边的千分之 permille 处出去，进到 to 那台电脑。返回是否成功
    fn enter(&mut self, link: &dyn Link, to: DeviceId, side: Side, permille: u16) -> bool {
        if !self.peers.contains_key(&to) { return false }
        let mods = self.held_mods.iter().fold(if self.caps { MOD_CAPS } else { 0 }, |m, u| m | modifier_bit(*u)) | if self.take_back { ASK_TAKE_BACK } else { 0 };
        let epoch = self.epoch.wrapping_add(1);
        if !link.event(to, Event::Enter { side: side.opposite(), permille, mods, epoch }) { return false }
        self.epoch = epoch;
        link.hold(to, true);
        self.mode = Mode::Remote { peer: to, epoch };
        self.carry = (0.0, 0.0);
        crate::ylog!("鼠标穿到 {}（第 {epoch} 次）", short(&to));
        true
    }

    /// 鼠标在本机时的移动：x、y 是现在的位置，dx、dy 是这次移动了多少
    pub fn local_move(&mut self, link: &dyn Link, x: f64, y: f64, dx: f64, dy: f64) -> Vec<Cmd> {
        let mut out = Vec::new();
        if self.mode != Mode::Local { return out }
        let (w, h) = self.size;
        self.local = (x.clamp(0.0, w - 1.0), y.clamp(0.0, h - 1.0));
        // 本机的鼠标把指针挪了：客人接着从这里走
        let mut step = dx.abs() + dy.abs();
        if let Some(g) = self.guest.as_mut() {
            // 有的设备不报移动量（触摸屏、手写笔）：看指针被它挪了多远（一次最多算 8，免得把客人刚挪的那一段也算进来）
            if step == 0.0 { step = ((self.local.0 - g.x).abs() + (self.local.1 - g.y).abs()).min(8.0) }
            g.x = self.local.0; g.y = self.local.1; g.shown = (g.x.round() as i32, g.y.round() as i32);
        }
        // 客人那头开着“鼠标被控切回”：本机的人真在用鼠标了（不是轻轻碰了一下），请它回去，指针留给本机
        if self.guest.as_ref().map_or(false, |g| g.ask_back) {
            if self.now.saturating_sub(self.nudge.0) > NUDGE_MS { self.nudge = (self.now, 0.0) }
            self.nudge.1 += step;
            if self.nudge.1 >= NUDGE_PX {
                if let Some(g) = self.guest.take() {
                    crate::ylog!("本机的鼠标动了，{} 开着“鼠标被控切回”：请它回去", short(&g.id));
                    link.event(g.id, Event::TakeBack { epoch: g.epoch });
                    out.push(left(1));
                }
                self.nudge = (self.now, 0.0);
            }
        }
        if self.buttons != 0 { return out }
        // 贴着哪条边、并且还在往外推
        let side = if x <= 0.0 && dx < 0.0 { Side::Left } else if x >= w - 1.0 && dx > 0.0 { Side::Right }
                   else if y <= 0.0 && dy < 0.0 { Side::Top } else if y >= h - 1.0 && dy > 0.0 { Side::Bottom } else { return out };
        let Some(to) = self.layout.neighbor(&self.me, side) else { return out };
        if !self.peers.contains_key(&to) { return out }
        let along = match side { Side::Left | Side::Right => self.local.1, _ => self.local.0 };
        // 回来的时候放在刚才出去的地方（往里一点，免得马上又穿出去）
        let before = self.local;
        self.local = entry(side, along, self.size);
        if !self.enter(link, to, side, to_permille(along, edge_length(side, self.size))) { self.local = before; return out }
        // 自己的鼠标穿出去了：正在本机上的客人请回去（一台电脑不同时既控制别人又被控制）。穿不成就不请
        if let Some(g) = self.guest.take() {
            crate::ylog!("本机的鼠标穿出去了，请 {} 回去", short(&g.id));
            link.event(g.id, Event::TakeBack { epoch: g.epoch });
            out.push(left(1));
        }
        out.push(Cmd::Grab { peer: to });
        out
    }

    /// 鼠标在别的电脑上时，本机鼠标的移动量：原样发过去（不足一个像素的攒着）
    pub fn remote_move(&mut self, link: &dyn Link, dx: f64, dy: f64) {
        let Mode::Remote { peer, .. } = self.mode else { return };
        self.carry = (self.carry.0 + dx, self.carry.1 + dy);
        let (ix, iy) = (self.carry.0.trunc(), self.carry.1.trunc());
        self.carry = (self.carry.0 - ix, self.carry.1 - iy);
        if ix != 0.0 || iy != 0.0 { link.motion(peer, ix as i32, iy as i32) }
    }

    /// 键盘的每一次按下、抬起。taken：外壳当时有没有把这个键从本机拿走——拿走的发给正在控制的那台，没拿走的是给了本机的
    pub fn key(&mut self, link: &dyn Link, usage: u16, down: bool, taken: bool) -> Option<Cmd> {
        if modifier_bit(usage) != 0 { if down { self.held_mods.insert(usage); } else { self.held_mods.remove(&usage); } }
        if !taken {
            // 穿走之前按下的修饰键穿屏时带过去了：这边松开，那边也要松开
            if let (false, Mode::Remote { peer, .. }, true) = (down, self.mode, modifier_bit(usage) != 0) { link.event(peer, Event::Key { usage, down: false }); }
            return None;
        }
        let Mode::Remote { peer, .. } = self.mode else { return None };          // 拿走的那一刻鼠标刚好回来了：这一下不要了
        // 修饰键记一笔（只记修饰键，别的键不记）：排查“Shift 没带过去”时两边的记录能对上
        if modifier_bit(usage) != 0 { crate::ylog!("修饰键 {usage:02X} {} → 发给 {}", if down { "按下" } else { "抬起" }, short(&peer)) }
        if link.event(peer, Event::Key { usage, down }) { None } else { self.go_home(link) }
    }
    /// 鼠标键（1 左、2 右、3 中……）。taken 同上
    pub fn button(&mut self, link: &dyn Link, button: u8, down: bool, taken: bool) -> Option<Cmd> {
        let bit = 1u32 << (button.min(31) as u32);
        if down { self.buttons |= bit } else { self.buttons &= !bit }
        if !taken { return None }
        let Mode::Remote { peer, .. } = self.mode else { return None };
        if link.event(peer, Event::Button { button, down }) { None } else { self.go_home(link) }
    }
    /// 滚轮。taken 同上
    pub fn wheel(&mut self, link: &dyn Link, dx: i32, dy: i32, taken: bool) {
        if let (true, Mode::Remote { peer, .. }) = (taken, self.mode) { link.wheel(peer, dx, dy) }
    }
    /// 不管鼠标在哪台电脑上，马上回到本机（紧急拉回）
    pub fn go_home(&mut self, link: &dyn Link) -> Option<Cmd> {
        let peer = self.remote()?;
        link.event(peer, Event::Leave); link.hold(peer, false);
        Some(self.home())
    }

    /// 别的电脑发来的一条输入。now 是现在的时间（毫秒，只用来算间隔）。返回要外壳按顺序做的事
    pub fn incoming(&mut self, link: &dyn Link, from: DeviceId, act: Action, now: u64) -> Vec<Cmd> {
        let mut out = Vec::new();
        self.now = now;
        let (w, h) = self.size;
        match act {
            Action::Event(Event::Enter { side, permille, mods, epoch }) => {
                // 谁最后穿过边谁控制：我的鼠标正在别处的话，先放开回本机
                if let Mode::Remote { peer, .. } = self.mode {
                    crate::ylog!("{} 的鼠标进来了，我的鼠标正在 {} 上：放开，回本机", short(&from), short(&peer));
                    link.event(peer, Event::Leave); link.hold(peer, false);
                    out.push(self.home());
                }
                if let Some(g) = self.guest.take() {
                    // 换一台来控制：请原来那台回去。同一台没说“走了”又进来一次也一样：先替上一次松开按着的键
                    if g.id != from { link.event(g.id, Event::TakeBack { epoch: g.epoch }); }
                    out.push(left(1));
                }
                let (x, y) = entry(side, from_permille(permille, edge_length(side, self.size)), self.size);
                let shown = (x.round() as i32, y.round() as i32);
                self.guest = Some(Guest { id: from, epoch, x, y, told: [None; 4], shown, ask_back: mods & ASK_TAKE_BACK != 0 });
                self.nudge = (now, 0.0);
                out.push(Cmd::Input { kind: 5, a: side as i32, b: permille as i32, c: (mods & !ASK_TAKE_BACK) as i32, d: 0 });
                out.push(Cmd::Input { kind: 1, a: shown.0, b: shown.1, c: 0, d: 0 });
            }
            Action::Event(Event::TakeBack { epoch }) => match self.mode {
                Mode::Remote { peer, epoch: e } if peer == from && e == epoch => {
                    crate::ylog!("{} 请我的鼠标回去", short(&from));
                    link.hold(from, false);
                    self.pull_in();
                    out.push(self.home());
                }
                _ => crate::ylog!("{} 的“回去吧”（第 {epoch} 次）过期了，不理", short(&from)),
            },
            Action::Event(Event::Edge { side, permille, epoch }) => {
                let Mode::Remote { peer, epoch: e } = self.mode else { return out };
                if peer != from || e != epoch || self.buttons != 0 { return out }       // 旧话不理；拖着东西时不穿
                match self.layout.neighbor(&from, side) {
                    Some(next) if next == self.me => {                              // 回到本机：从对方出去的地方按比例对回来
                        link.event(from, Event::Leave); link.hold(from, false);
                        let came_in = side.opposite();
                        self.local = entry(came_in, from_permille(permille, edge_length(came_in, self.size)), self.size);
                        out.push(self.home());
                    }
                    Some(next) if self.peers.contains_key(&next) => {               // 接着穿到第三台
                        link.event(from, Event::Leave); link.hold(from, false);
                        out.push(if self.enter(link, next, side, permille) { Cmd::Grab { peer: next } } else { self.home() });
                    }
                    _ => {}                                                         // 那边没有电脑：指针就停在它的边上
                }
            }
            // 下面几种只认正在本机上的那位客人发的；别的电脑发的、它走了以后晚到的，都不理
            other => {
                let Some(g) = self.guest.as_mut().filter(|g| g.id == from) else { return out };
                match other {
                    Action::Motion { dx, dy } => {
                        let (nx, ny) = (g.x + dx as f64, g.y + dy as f64);
                        let sides = [if nx < 0.0 { Some(Side::Left) } else if nx > w - 1.0 { Some(Side::Right) } else { None },
                                     if ny < 0.0 { Some(Side::Top) } else if ny > h - 1.0 { Some(Side::Bottom) } else { None }];
                        g.x = nx.clamp(0.0, w - 1.0); g.y = ny.clamp(0.0, h - 1.0);
                        // 走到边了：告诉它是哪条边的什么位置，怎么办由它定（顶在角上两条边都说，那边没电脑的它自己会不理）。
                        // 顶着同一条边不动时别每一下都说；四条边各记各的，刚说过这条不耽误说那条
                        for side in sides.into_iter().flatten() {
                            let told = &mut g.told[side as usize];
                            if told.map_or(true, |t| now.saturating_sub(t) >= EDGE_REPEAT_MS) {
                                *told = Some(now);
                                let along = match side { Side::Left | Side::Right => g.y, _ => g.x };
                                link.event(from, Event::Edge { side, permille: to_permille(along, edge_length(side, (w, h))), epoch: g.epoch });
                            }
                        }
                        let at = (g.x.round() as i32, g.y.round() as i32);
                        if at != g.shown { g.shown = at; out.push(Cmd::Input { kind: 1, a: at.0, b: at.1, c: 0, d: 0 }) }
                    }
                    Action::Wheel { dx, dy } => out.push(Cmd::Input { kind: 2, a: dx, b: dy, c: 0, d: 0 }),
                    Action::Event(Event::Key { usage, down }) => {
                        if modifier_bit(usage) != 0 { crate::ylog!("{} 的修饰键 {usage:02X} {}", short(&from), if down { "按下" } else { "抬起" }) }
                        out.push(Cmd::Input { kind: 3, a: usage as i32, b: down as i32, c: 0, d: 0 })
                    }
                    Action::Event(Event::Button { button, down }) => out.push(Cmd::Input { kind: 4, a: button as i32, b: down as i32, c: g.shown.0, d: g.shown.1 }),
                    Action::Event(Event::Leave) => { self.guest = None; out.push(left(0)) }
                    _ => {}
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Clone, PartialEq)]
    enum Sent { Event(DeviceId, Event), Motion(DeviceId, i32, i32), Wheel(DeviceId, i32, i32), Hold(DeviceId, bool) }
    #[derive(Default)]
    struct Fake { log: RefCell<Vec<Sent>>, dead: RefCell<Vec<DeviceId>> }
    impl Link for Fake {
        fn event(&self, id: DeviceId, ev: Event) -> bool { if self.dead.borrow().contains(&id) { return false }; self.log.borrow_mut().push(Sent::Event(id, ev)); true }
        fn motion(&self, id: DeviceId, dx: i32, dy: i32) { self.log.borrow_mut().push(Sent::Motion(id, dx, dy)) }
        fn wheel(&self, id: DeviceId, dx: i32, dy: i32) { self.log.borrow_mut().push(Sent::Wheel(id, dx, dy)) }
        fn hold(&self, id: DeviceId, on: bool) { self.log.borrow_mut().push(Sent::Hold(id, on)) }
    }
    impl Fake { fn take(&self) -> Vec<Sent> { std::mem::take(&mut *self.log.borrow_mut()) } }

    const MAC: DeviceId = [1; 16];
    const WIN: DeviceId = [2; 16];
    const LINUX: DeviceId = [3; 16];

    /// Mac（1920×1080）在中间，Windows（2560×1440）在它右边
    fn setup() -> (Controller, Fake) {
        let mut c = Controller::new(MAC, 1920, 1080);
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(WIN, 1, 0);
        c.set_layout(l);
        c.peer_connected(WIN, 2560, 1440);
        (c, Fake::default())
    }
    /// 把鼠标推出右边缘，穿到 Windows 上
    fn cross(c: &mut Controller, link: &Fake) { assert_eq!(c.local_move(link, 1919.0, 540.0, 4.0, 0.0), vec![Cmd::Grab { peer: WIN }]); link.take(); }
    /// Windows 的鼠标从 Mac 的右边缘中间进来（它的第 epoch 次）
    fn enter(epoch: u32) -> Action { Action::Event(Event::Enter { side: Side::Right, permille: 500, mods: 0, epoch }) }
    fn input(kind: i32, a: i32, b: i32, c: i32, d: i32) -> Cmd { Cmd::Input { kind, a, b, c, d } }
    fn edge(side: Side, permille: u16, epoch: u32) -> Action { Action::Event(Event::Edge { side, permille, epoch }) }

    // —— 自己的鼠标穿出去 ——

    #[test]
    fn 推到右边缘就穿到右边那台() {
        let (mut c, link) = setup();
        assert_eq!(c.local_move(&link, 900.0, 540.0, 5.0, 0.0), vec![]);                  // 屏幕中间：没事
        assert_eq!(c.local_move(&link, 1919.0, 539.5, 0.0, 3.0), vec![]);                 // 贴着边但没往外推：没事
        assert_eq!(c.local_move(&link, 1919.0, 539.5, 4.0, 0.0), vec![Cmd::Grab { peer: WIN }]);
        assert_eq!(c.remote(), Some(WIN));
        // 从 Mac 的右边中间出去：告诉对方“从你的左边、千分之五百处进来”，落在哪由对方按自己的屏幕算
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 500, mods: 0, epoch: 1 }), Sent::Hold(WIN, true)]);
    }

    #[test]
    fn 在那边移动点击打字都发过去() {
        let (mut c, link) = setup();
        cross(&mut c, &link);
        c.remote_move(&link, 100.0, -20.0);
        assert_eq!(c.button(&link, 1, true, true), None);
        assert_eq!(c.button(&link, 1, false, true), None);
        assert_eq!(c.key(&link, 0x04, true, true), None);
        c.wheel(&link, 0, -3, true);
        assert_eq!(link.take(), vec![
            Sent::Motion(WIN, 100, -20),
            Sent::Event(WIN, Event::Button { button: 1, down: true }),
            Sent::Event(WIN, Event::Button { button: 1, down: false }),
            Sent::Event(WIN, Event::Key { usage: 0x04, down: true }),
            Sent::Wheel(WIN, 0, -3),
        ]);
    }

    #[test]
    fn 不足一个像素的移动量攒着发() {
        let (mut c, link) = setup();
        cross(&mut c, &link);
        c.remote_move(&link, 0.4, -0.4); c.remote_move(&link, 0.4, -0.4);
        assert_eq!(link.take(), vec![]);
        c.remote_move(&link, 0.4, -0.4);
        c.remote_move(&link, 2.5, 0.0);
        assert_eq!(link.take(), vec![Sent::Motion(WIN, 1, -1), Sent::Motion(WIN, 2, 0)]);
        c.remote_move(&link, 5.0, 5.0);                                                 // 鼠标在本机时不发
        let (mut c, link) = setup();
        c.remote_move(&link, 5.0, 5.0);
        assert_eq!(link.take(), vec![]);
    }

    #[test]
    fn 对方说鼠标走到靠这边的边了_就回来() {
        let (mut c, link) = setup();
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 500, 1), 0), vec![]);           // 鼠标本来就在本机
        cross(&mut c, &link);
        assert_eq!(c.incoming(&link, LINUX, edge(Side::Left, 500, 1), 0), vec![]);         // 不是正在控制的那台说的
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 500, 9), 0), vec![]);           // 序号对不上：旧话
        assert_eq!(c.incoming(&link, WIN, edge(Side::Right, 500, 1), 0), vec![]);          // 它的右边没有电脑：不动
        assert_eq!(c.incoming(&link, WIN, edge(Side::Top, 0, 1), 0), vec![]);
        assert_eq!(c.remote(), Some(WIN));
        assert_eq!(link.take(), vec![]);
        // 走到它的左边缘、四分之一高的地方：回到 Mac 右边缘四分之一高处
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 250, 1), 0), vec![Cmd::Release { x: 1918, y: 270 }]);
        assert_eq!(c.remote(), None);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Leave), Sent::Hold(WIN, false)]);
        assert_eq!(c.key(&link, 0x04, true, false), None);                              // 回来之后按键归本机，什么都不发
        assert_eq!(c.key(&link, 0x05, true, true), None);                               // 回来那一刻被外壳拿走的键：不要了，也不发
        c.wheel(&link, 0, 1, false);
        assert_eq!(link.take(), vec![]);
    }

    #[test]
    fn 按着本机的键穿过去再松开_松开留给本机() {
        let (mut c, link) = setup();
        assert_eq!(c.key(&link, 0xE1, true, false), None);                              // 本机按住左 Shift
        assert_eq!(c.key(&link, 0x04, true, false), None);                              // 本机还按着一个普通键
        c.local_move(&link, 1919.0, 540.0, 4.0, 0.0);
        assert_eq!(link.take()[0], Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 500, mods: 1, epoch: 1 }));
        assert_eq!(c.key(&link, 0xE1, false, false), None);                             // Shift 的松开给本机，也告诉那边（穿屏时带过去了）
        assert_eq!(c.key(&link, 0x04, false, false), None);                             // 普通键没带过去：松开只给本机
        assert_eq!(c.key(&link, 0x05, true, true), None);
        assert_eq!(c.key(&link, 0x05, false, true), None);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Key { usage: 0xE1, down: false }),
                                     Sent::Event(WIN, Event::Key { usage: 0x05, down: true }), Sent::Event(WIN, Event::Key { usage: 0x05, down: false })]);
    }

    #[test]
    fn 左右修饰键分开记() {
        let (mut c, link) = setup();
        c.key(&link, 0xE1, true, false); c.key(&link, 0xE5, true, false); c.key(&link, 0xE5, false, false);  // 按左 Shift、右 Shift，松开右边
        c.local_move(&link, 1919.0, 0.0, 4.0, 0.0);
        assert_eq!(link.take()[0], Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 0, mods: crate::keys::MOD_SHIFT, epoch: 1 }));
    }

    #[test]
    fn 发不出去就回本机() {
        let (mut c, link) = setup();
        cross(&mut c, &link);
        link.dead.borrow_mut().push(WIN);
        assert_eq!(c.key(&link, 0x04, true, true), Some(Cmd::Release { x: 1918, y: 540 }));
        assert_eq!(c.remote(), None);
        assert_eq!(link.take(), vec![Sent::Hold(WIN, false)]);                          // “走了”发不出去，至少不再当自己控制着它
        let (mut c, link) = setup();
        link.dead.borrow_mut().push(WIN);
        assert_eq!(c.local_move(&link, 1919.0, 300.0, 4.0, 0.0), vec![]);                  // 一开始就发不出去：不穿
        assert_eq!(c.remote(), None);
        assert_eq!(c.local_move(&link, 1000.0, 300.0, -4.0, 0.0), vec![]);
    }

    #[test]
    fn 对方请我回去就回来_过期的话不理() {
        let (mut c, link) = setup();
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::TakeBack { epoch: 1 }), 0), vec![]);      // 本来就在本机
        cross(&mut c, &link);
        assert_eq!(c.incoming(&link, LINUX, Action::Event(Event::TakeBack { epoch: 1 }), 0), vec![]);    // 不是正在控制的那台说的
        // 被请回来的：落点离边远一点（往里 40），不会手一蹭又穿过去
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::TakeBack { epoch: 1 }), 0), vec![Cmd::Release { x: 1879, y: 540 }]);
        assert_eq!(c.remote(), None);
        assert_eq!(link.take(), vec![Sent::Hold(WIN, false)]);
        // 又穿过去一次（第 2 次）。这时才到的“第 1 次回去吧”是旧话
        cross(&mut c, &link);
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::TakeBack { epoch: 1 }), 0), vec![]);
        assert_eq!(c.remote(), Some(WIN));
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::TakeBack { epoch: 2 }), 0), vec![Cmd::Release { x: 1879, y: 540 }]);
    }

    #[test]
    fn 紧急拉回() {
        let (mut c, link) = setup();
        assert_eq!(c.go_home(&link), None);                                             // 本来就在本机
        cross(&mut c, &link);
        assert_eq!(c.go_home(&link), Some(Cmd::Release { x: 1918, y: 540 }));
        assert_eq!(c.remote(), None);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Leave), Sent::Hold(WIN, false)]);
    }

    #[test]
    fn 没有邻居或者邻居没连上的边不会穿() {
        let (mut c, link) = setup();
        assert_eq!(c.local_move(&link, 0.0, 500.0, -9.0, 0.0), vec![]);                   // 左边没电脑
        assert_eq!(c.local_move(&link, 800.0, 0.0, 0.0, -9.0), vec![]);                   // 上面没电脑
        assert_eq!(c.peer_disconnected(WIN), vec![]);
        assert_eq!(c.local_move(&link, 1919.0, 300.0, 4.0, 0.0), vec![]);                 // 右边那台没连着
        assert_eq!(link.take(), vec![]);
    }

    #[test]
    fn 按着鼠标键拖动时不穿屏() {
        let (mut c, link) = setup();
        c.button(&link, 1, true, false);
        assert_eq!(c.local_move(&link, 1919.0, 540.0, 4.0, 0.0), vec![]);
        c.button(&link, 1, false, false);
        cross(&mut c, &link);
        c.button(&link, 1, true, true);                                                 // 在那边拖着东西走到了边：也不回来
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 500, 1), 0), vec![]);
        c.button(&link, 1, false, true);
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 500, 1), 0), vec![Cmd::Release { x: 1918, y: 540 }]);
    }

    #[test]
    fn 三台连成一排可以一路穿过去再回来() {
        let mut c = Controller::new(MAC, 1920, 1080);
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(WIN, 1, 0); l.place(LINUX, 2, 0);
        c.set_layout(l);
        c.peer_connected(WIN, 2560, 1440); c.peer_connected(LINUX, 1920, 1200);
        let link = Fake::default();
        cross(&mut c, &link);
        // Windows 说鼠标到它右边缘了：右边是 Linux，接着穿过去
        assert_eq!(c.incoming(&link, WIN, edge(Side::Right, 300, 1), 0), vec![Cmd::Grab { peer: LINUX }]);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Leave), Sent::Hold(WIN, false),
                                     Sent::Event(LINUX, Event::Enter { side: Side::Left, permille: 300, mods: 0, epoch: 2 }), Sent::Hold(LINUX, true)]);
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 300, 1), 0), vec![], "Windows 晚到的话不理");
        assert_eq!(c.incoming(&link, LINUX, edge(Side::Left, 300, 2), 0), vec![Cmd::Grab { peer: WIN }]);    // 往回：先到 Windows
        link.take();
        assert_eq!(c.incoming(&link, WIN, edge(Side::Left, 500, 3), 0), vec![Cmd::Release { x: 1918, y: 540 }]);
    }

    #[test]
    fn 上下摆也一样() {
        let mut c = Controller::new(MAC, 1920, 1080);
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(LINUX, 0, 1);                                       // Linux 在 Mac 下面
        c.set_layout(l);
        c.peer_connected(LINUX, 3840, 2160);
        let link = Fake::default();
        assert_eq!(c.local_move(&link, 480.0, 1079.0, 0.0, 6.0), vec![Cmd::Grab { peer: LINUX }]);
        assert_eq!(link.take(), vec![Sent::Event(LINUX, Event::Enter { side: Side::Top, permille: 250, mods: 0, epoch: 1 }), Sent::Hold(LINUX, true)]);
        assert_eq!(c.incoming(&link, LINUX, edge(Side::Top, 250, 1), 0), vec![Cmd::Release { x: 480, y: 1078 }]);
    }

    #[test]
    fn 外壳报来的位置越出屏幕也不出错() {
        let (mut c, link) = setup();
        // Windows 的钩子在顶着边推时给的是“想去的位置”，可能在屏幕外面
        assert_eq!(c.local_move(&link, 1925.0, -30.0, 6.0, -2.0), vec![Cmd::Grab { peer: WIN }]);
        assert_eq!(link.take()[0], Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 0, mods: 0, epoch: 1 }));
        assert_eq!(c.go_home(&link), Some(Cmd::Release { x: 1918, y: 0 }));
    }

    #[test]
    fn 对方断线时鼠标回到本机() {
        let (mut c, link) = setup();
        c.local_move(&link, 1919.0, 300.0, 4.0, 0.0);
        assert_eq!(c.peer_disconnected(WIN), vec![Cmd::Release { x: 1918, y: 300 }]);
        assert_eq!(c.remote(), None);
    }

    // —— 别的电脑的鼠标进到本机 ——

    #[test]
    fn 别的电脑的鼠标进来_放到入口_照它的移动量走() {
        let (mut c, link) = setup();
        // 没进来过：谁发来的输入都不照做（比如它走了以后还在路上的几个包）
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 5, dy: 5 }, 0), vec![]);
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::Key { usage: 4, down: true }), 0), vec![]);
        // 从 Mac 的右边缘中间进来：指针放到那里
        assert_eq!(c.incoming(&link, WIN, enter(7), 0), vec![input(5, 1, 500, 0, 0), input(1, 1918, 540, 0, 0)]);
        assert!(c.hosting());
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: -100, dy: 20 }, 0), vec![input(1, 1818, 560, 0, 0)]);
        assert_eq!(c.incoming(&link, WIN, Action::Wheel { dx: 0, dy: -120 }, 0), vec![input(2, 0, -120, 0, 0)]);
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::Key { usage: 4, down: true }), 0), vec![input(3, 4, 1, 0, 0)]);
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::Button { button: 1, down: true }), 0), vec![input(4, 1, 1, 1818, 560)]);
        assert_eq!(c.incoming(&link, LINUX, Action::Motion { dx: 1, dy: 1 }, 0), vec![]);      // 别的电脑插嘴：不理
        assert_eq!(c.incoming(&link, LINUX, Action::Event(Event::Leave), 0), vec![]);
        assert!(c.hosting());
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::Leave), 0), vec![input(6, 0, 0, 0, 0)]);
        assert!(!c.hosting());
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 5, dy: 7 }, 0), vec![]);        // 走了以后晚到的包
        assert_eq!(link.take(), vec![]);
    }

    #[test]
    fn 客人的鼠标走到边就告诉它_顶着不动时不每一下都说() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 1000);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: -100, dy: 20 }, 1000), vec![input(1, 1818, 560, 0, 0)]);
        assert_eq!(link.take(), vec![]);
        // 往右走过头：指针停在右边缘，告诉它“到我的右边了，千分之 519 处”
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 500, dy: 0 }, 1010), vec![input(1, 1919, 560, 0, 0)]);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Edge { side: Side::Right, permille: 519, epoch: 7 })]);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 30, dy: 0 }, 1050), vec![], "还顶着：指针没动，也不重复说");
        assert_eq!(link.take(), vec![]);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 30, dy: 4 }, 1120), vec![input(1, 1919, 564, 0, 0)]);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Edge { side: Side::Right, permille: 522, epoch: 7 })], "过了一百毫秒还顶着：再说一次");
        // 顶到左上角
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: -5000, dy: -5000 }, 2000), vec![input(1, 0, 0, 0, 0)]);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Edge { side: Side::Left, permille: 0, epoch: 7 }), Sent::Event(WIN, Event::Edge { side: Side::Top, permille: 0, epoch: 7 })],
                   "顶在角上：两条边都说");
    }

    #[test]
    fn 刚说过这条边不耽误说另一条() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 1000);
        // 先顶到上边（那边没有电脑，Windows 不会理），紧接着沿着上边滑到右边：到了右边马上说，不等一百毫秒
        c.incoming(&link, WIN, Action::Motion { dx: -50, dy: -2000 }, 1000);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Edge { side: Side::Top, permille: 973, epoch: 7 })]);
        c.incoming(&link, WIN, Action::Motion { dx: 60, dy: -3 }, 1008);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Edge { side: Side::Right, permille: 0, epoch: 7 })], "上边刚说过不再说，右边照说");
    }

    #[test]
    fn 本机的鼠标和客人的鼠标动的是同一个指针() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 0);
        // 本机自己的鼠标把指针挪到了 (300, 300)，还按了键、点了一下：客人不会被赶走
        assert_eq!(c.local_move(&link, 300.0, 300.0, 5.0, 0.0), vec![]);
        assert_eq!(c.key(&link, 0x04, true, false), None);
        assert_eq!(c.button(&link, 1, true, false), None);
        assert_eq!(c.button(&link, 1, false, false), None);
        assert!(c.hosting());
        assert_eq!(link.take(), vec![]);
        // 客人接着从指针现在的地方走
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 10, dy: -5 }, 0), vec![input(1, 310, 295, 0, 0)]);
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::Button { button: 1, down: true }), 0), vec![input(4, 1, 1, 310, 295)]);
    }

    #[test]
    fn 自己的鼠标要穿出去时_先请客人回去() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 0);
        // 本机的鼠标推出右边缘（那边正是客人那台）：请它回去，然后我的鼠标过去
        assert_eq!(c.local_move(&link, 1919.0, 540.0, 4.0, 0.0), vec![input(6, 1, 0, 0, 0), Cmd::Grab { peer: WIN }]);
        assert!(!c.hosting());
        assert_eq!(c.remote(), Some(WIN));
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 500, mods: 0, epoch: 1 }), Sent::Hold(WIN, true),
                                     Sent::Event(WIN, Event::TakeBack { epoch: 7 })]);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 3, dy: 3 }, 0), vec![], "它还在路上的包不理");
    }

    #[test]
    fn 我的鼠标正在别处时别的电脑进来_我先放开回本机() {
        let (mut c, link) = setup();
        cross(&mut c, &link);
        assert_eq!(c.incoming(&link, WIN, enter(5), 0), vec![Cmd::Release { x: 1918, y: 540 }, input(5, 1, 500, 0, 0), input(1, 1918, 540, 0, 0)]);
        assert_eq!(c.remote(), None);
        assert!(c.hosting());
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::Leave), Sent::Hold(WIN, false)]);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: -9, dy: 0 }, 0), vec![input(1, 1909, 540, 0, 0)]);
    }

    #[test]
    fn 本机的鼠标穿不出去时_客人留着() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 0);
        link.take();
        link.dead.borrow_mut().push(WIN);
        assert_eq!(c.local_move(&link, 1919.0, 540.0, 4.0, 0.0), vec![]);
        assert!(c.hosting() && c.remote().is_none());
    }

    #[test]
    fn 两台同时把鼠标推向对方_两边都回到本机() {
        let mut l = Layout::default(); l.place(MAC, 0, 0); l.place(WIN, 1, 0);
        let mut a = Controller::new(MAC, 1920, 1080); a.set_layout(l.clone()); a.peer_connected(WIN, 2560, 1440);
        let mut b = Controller::new(WIN, 2560, 1440); b.set_layout(l); b.peer_connected(MAC, 1920, 1080);
        let (la, lb) = (Fake::default(), Fake::default());
        assert_eq!(a.local_move(&la, 1919.0, 540.0, 4.0, 0.0), vec![Cmd::Grab { peer: WIN }]);
        assert_eq!(b.local_move(&lb, 0.0, 720.0, -4.0, 0.0), vec![Cmd::Grab { peer: MAC }]);
        // 两边的话互相送到，直到都没话说
        loop {
            let (sa, sb) = (la.take(), lb.take());
            if sa.is_empty() && sb.is_empty() { break }
            for m in sa { if let Sent::Event(_, e) = m { b.incoming(&lb, MAC, Action::Event(e), 0); } }
            for m in sb { if let Sent::Event(_, e) = m { a.incoming(&la, WIN, Action::Event(e), 0); } }
        }
        // 谁也没抢到，但两边说法一致、都能接着用：再推一次就过去了
        assert!(a.remote().is_none() && !a.hosting() && b.remote().is_none() && !b.hosting());
        assert_eq!(a.local_move(&la, 1919.0, 540.0, 4.0, 0.0), vec![Cmd::Grab { peer: WIN }]);
    }

    #[test]
    fn 另一台进来就换它_同一台没说走了又进来就重新算() {
        let (mut c, link) = setup();
        c.peer_connected(LINUX, 1920, 1200);
        c.incoming(&link, WIN, enter(7), 0);
        assert_eq!(c.incoming(&link, LINUX, enter(2), 0), vec![input(6, 1, 0, 0, 0), input(5, 1, 500, 0, 0), input(1, 1918, 540, 0, 0)]);
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::TakeBack { epoch: 7 })]);
        assert_eq!(c.incoming(&link, WIN, Action::Motion { dx: 1, dy: 1 }, 0), vec![]);
        assert_eq!(c.incoming(&link, LINUX, Action::Motion { dx: -2, dy: 2 }, 0), vec![input(1, 1916, 542, 0, 0)]);
        // 同一台没说“走了”又进来一次：先替上一次松开，再算新的一次；之后“到边了”带的是新的序号
        assert_eq!(c.incoming(&link, LINUX, enter(3), 0), vec![input(6, 1, 0, 0, 0), input(5, 1, 500, 0, 0), input(1, 1918, 540, 0, 0)]);
        c.incoming(&link, LINUX, Action::Motion { dx: 50, dy: 0 }, 0);
        assert_eq!(link.take(), vec![Sent::Event(LINUX, Event::Edge { side: Side::Right, permille: 500, epoch: 3 })]);
    }

    #[test]
    fn 客人那台掉线_替它松开() {
        let (mut c, link) = setup();
        c.incoming(&link, WIN, enter(7), 0);
        assert_eq!(c.peer_disconnected(LINUX), vec![]);
        assert_eq!(c.peer_disconnected(WIN), vec![input(6, 2, 0, 0, 0)]);
        assert!(!c.hosting());
        assert_eq!(c.peer_disconnected(WIN), vec![]);
    }

    #[test]
    fn 鼠标被控切回_客人那头开着_本机的鼠标真动了就请它回去_轻碰不算() {
        let (mut c, link) = setup();
        let asked = |epoch| Action::Event(Event::Enter { side: Side::Right, permille: 500, mods: ASK_TAKE_BACK | 1, epoch });
        // 客人那头没开：本机的鼠标怎么动，客人都还在——哪怕本机自己的开关开着（开关只管本机去控制别人的时候）
        c.set_take_back(true);
        c.incoming(&link, WIN, enter(7), 0); link.take();
        assert_eq!(c.local_move(&link, 500.0, 500.0, 80.0, 0.0), vec![]);
        assert!(c.hosting());
        c.set_take_back(false);
        // 客人那头开着（它穿过来时说了）。这句话不当成修饰键交给外壳
        assert_eq!(c.incoming(&link, WIN, asked(8), 900)[1], input(5, 1, 500, 1, 0));
        link.take();
        // 轻轻碰一下（合起来不到 30 像素）不算；隔了很久的两下不加在一起
        c.set_now(1000); assert_eq!(c.local_move(&link, 510.0, 500.0, 10.0, 0.0), vec![]);
        c.set_now(2000); assert_eq!(c.local_move(&link, 530.0, 500.0, 20.0, 0.0), vec![]);
        assert!(c.hosting());
        // 紧接着又动：这台的人真在用鼠标了，请它回去，指针留给本机
        c.set_now(2100); assert_eq!(c.local_move(&link, 545.0, 500.0, 15.0, 0.0), vec![input(6, 1, 0, 0, 0)]);
        assert!(!c.hosting());
        assert_eq!(link.take(), vec![Sent::Event(WIN, Event::TakeBack { epoch: 8 })]);
        assert_eq!(c.local_move(&link, 600.0, 500.0, 55.0, 0.0), vec![]);       // 没有客人了：什么都不发生
        assert_eq!(link.take(), vec![]);
        // 它再穿过来照样能进。不报移动量的设备（触摸屏、手写笔）：看指针被挪了多远，一次最多算 8
        assert_eq!(c.incoming(&link, WIN, asked(9), 3000).len(), 2);
        c.set_now(3100);
        for i in 1..=3 { assert_eq!(c.local_move(&link, 1000.0 - 100.0 * i as f64, 540.0, 0.0, 0.0), vec![]); }
        assert_eq!(c.local_move(&link, 600.0, 540.0, 0.0, 0.0), vec![input(6, 1, 0, 0, 0)]);
        link.take();
        // 本机开着的时候，自己的鼠标穿到别的电脑上会带上这句话；关着就不带
        c.set_take_back(true);
        c.local_move(&link, 1919.0, 540.0, 4.0, 0.0);
        assert_eq!(link.take()[0], Sent::Event(WIN, Event::Enter { side: Side::Left, permille: 500, mods: ASK_TAKE_BACK, epoch: 1 }));
        // 被请回来：落在出去的那条边往里一点（和别的电脑来接手时一样）
        assert_eq!(c.incoming(&link, WIN, Action::Event(Event::TakeBack { epoch: 1 }), 4000), vec![Cmd::Release { x: 1879, y: 540 }]);
    }
}
