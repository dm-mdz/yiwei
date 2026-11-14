//! 输入通道：鼠标和按键怎么在容易丢包的网络（无线网）上传，既不卡也不丢。
//!
//! 走 UDP，丢了包不等重传。靠三条规矩保证结果正确：
//!   · 鼠标移动：发的是移动量，但每个包带的是“从开始到现在一共移了多少”，接收方自己减出这次该移多少，
//!     中间丢几个包都不会少走。鼠标在动的时候不要对方逐包确认；停下来以后最后那个数要确认，没确认就补发。
//!   · 滚轮：和鼠标移动一样，带累计量。
//!   · 按键、鼠标点击、穿屏：一次都不能丢，顺序也不能乱。每个包都把“对方还没确认收到的那几次”重新带上，
//!     对方确认后才不再带；没事件要发时也会每隔十几毫秒补发，直到确认。
//!     每个事件还带着它发生那一刻的鼠标累计移动量：接收方先把鼠标走到那里再做这个事件，点击不会点歪。
//!
//! 这里只有收发两头的状态和包的格式，不碰网络也不碰时钟（时间由调用方传进来），所以能用模拟丢包来测。
//! 加密在这一层外面做。

use crate::wire::{Reader, Side, WireError, Writer};
use std::collections::VecDeque;

/// 有没确认的事件时，隔这么久补发一次（毫秒）
pub const RESEND_MS: u64 = 12;
/// 鼠标停下后，最后的累计量没确认时隔这么久补发一次（毫秒）。比上面长：鼠标慢慢动（两次移动隔十几毫秒）时
/// 下一次移动马上就带着新的累计量来了，用不着每动一下都补发、都要确认
pub const MOTION_RESEND_MS: u64 = 40;
/// 正在控制对方时，没事也隔这么久发一个包，让对方知道连接还在（毫秒）
pub const HEARTBEAT_MS: u64 = 250;
/// 最多攒这么多没确认的事件。攒满说明对方已经很久没回应，该当成断线处理
pub const MAX_UNACKED: usize = 40;
/// “走了”“回去吧”“到边了”这几句不受上面的限制（不然攒满时连“走了”都说不出去，对方会一直替这边按着键），另留这么多位置
const MAX_PARTING: usize = 16;
/// 一个事件等了这么久（毫秒）对方还没确认：输入通道不通（多半是对方的防火墙只放行了一半），该断开重连
pub const STALL_MS: u64 = 1500;

/// 一次都不能丢的事件
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Key { usage: u16, down: bool },
    /// 鼠标键
    Button { button: u8, down: bool },
    /// 鼠标穿到对方屏幕上了：从哪条边、沿边的千分之几处进来。epoch：发的一方第几次把鼠标穿出去
    Enter { side: Side, permille: u16, mods: u32, epoch: u32 },
    /// 鼠标回来了，对方松开所有按键
    Leave,
    /// 被控的一方发给控制方：你的鼠标回去吧（别的电脑接手了，或者我自己的鼠标要穿出去了）。
    /// epoch 是它那次 Enter 带的数；对不上说明是晚到的旧话，收的一方不理
    TakeBack { epoch: u32 },
    /// 被控的一方发给控制方：你的鼠标走到我屏幕的这条边了（沿边的千分之几处）。要不要回去、要不要接着穿到下一台，由控制方按摆法定
    Edge { side: Side, permille: u16, epoch: u32 },
}

/// 收到一个包之后，该做的事（按顺序做）
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Event(Event),
    Wheel { dx: i32, dy: i32 },
    /// 鼠标移动了这么多
    Motion { dx: i32, dy: i32 },
}

/// 连接的一头。两头各有一个，既发也收。
#[derive(Default)]
pub struct Endpoint {
    // 发
    seq: u64,
    next_id: u32,
    unacked: VecDeque<(u32, Event, (i32, i32))>,   // 事件、它发生那一刻的鼠标累计移动量
    active: bool,               // 正控制着对方
    motion: (i32, i32),
    wheel: (i32, i32),
    dirty: bool,
    state_dirty: bool,          // 鼠标移动或滚轮有变化，还没发
    state_seq: u64,             // 最新的累计量是在第几个包里第一次发出去的
    acked_state: u64,           // 对方确认已经收到的是哪一个
    last_send: Option<u64>,
    waiting_since: Option<u64>, // 最老的那个没确认的事件是从什么时候开始等的
    // 收
    peer_seq: u64,
    peer_state: u64,
    applied: u32,
    peer_motion: (i32, i32),
    peer_wheel: (i32, i32),
    ack_dirty: bool,
}

fn side_of(n: u8) -> Result<Side, WireError> { Ok(match n { 0 => Side::Left, 1 => Side::Right, 2 => Side::Top, 3 => Side::Bottom, _ => return Err(WireError::BadValue) }) }

impl Endpoint {
    pub fn new() -> Self { Self::default() }

    /// 排进一个不能丢的事件。返回 false 表示攒满了（对方太久没回应），调用方应当断开重连。
    pub fn push(&mut self, ev: Event) -> bool {
        let parting = matches!(ev, Event::Leave | Event::TakeBack { .. } | Event::Edge { .. });
        if self.unacked.len() >= MAX_UNACKED + if parting { MAX_PARTING } else { 0 } { return false }
        self.next_id += 1;
        self.unacked.push_back((self.next_id, ev, self.motion));
        self.dirty = true;
        true
    }
    /// 鼠标又移动了这么多
    pub fn motion(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 { return }
        self.motion = (self.motion.0.wrapping_add(dx), self.motion.1.wrapping_add(dy));
        self.dirty = true; self.state_dirty = true;
    }
    pub fn wheel(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 { return }
        self.wheel = (self.wheel.0.wrapping_add(dx), self.wheel.1.wrapping_add(dy));
        self.dirty = true; self.state_dirty = true;
    }
    /// 开始 / 不再控制对方（控制着的时候隔一会儿报个平安）
    pub fn hold(&mut self, on: bool) { self.active = on }
    /// 是不是正控制着对方
    pub fn active(&self) -> bool { self.active }
    /// 有事件等确认等得太久了没有（见 STALL_MS）。报一次就重新计时
    pub fn take_stalled(&mut self, now_ms: u64) -> bool {
        let stalled = self.waiting_since.map_or(false, |t| now_ms.saturating_sub(t) > STALL_MS);
        if stalled { self.waiting_since = Some(now_ms) }
        stalled
    }
    /// 还有多少东西等对方确认（事件个数；累计量没确认也算一个）
    pub fn pending(&self) -> usize { self.unacked.len() + (self.state_dirty || self.state_seq > self.acked_state) as usize }

    /// 现在该不该发包；该发就返回包的内容（明文，外面再加密）。调用方每隔几毫秒、以及每次有新输入后调一次。
    pub fn poll(&mut self, now_ms: u64) -> Option<Vec<u8>> {
        if self.unacked.is_empty() { self.waiting_since = None } else if self.waiting_since.is_none() { self.waiting_since = Some(now_ms) }
        let since = self.last_send.map(|t| now_ms.saturating_sub(t));
        let resend = (!self.unacked.is_empty() && since.map_or(true, |d| d >= RESEND_MS))
                  || (self.state_seq > self.acked_state && since.map_or(true, |d| d >= MOTION_RESEND_MS));
        let heartbeat = self.active && since.map_or(true, |d| d >= HEARTBEAT_MS);
        if !(self.dirty || self.ack_dirty || resend || heartbeat) { return None }
        self.seq += 1;
        let fresh = self.state_dirty;
        if self.state_dirty { self.state_seq = self.seq; self.state_dirty = false }
        // 要不要对方回一个包：有事件没确认，或者累计量没确认而且这个包里它没再变（鼠标停了、在补发）。
        // 鼠标正在动的时候不要确认：下一个包马上就来，逐包确认只是白白多一倍的包
        let wants_ack = !self.unacked.is_empty() || (self.state_seq > self.acked_state && !fresh);
        let mut w = Writer(Vec::with_capacity(64));
        w.u64(self.seq);
        w.u32(self.applied);
        w.u64(self.peer_state);
        w.u64(self.state_seq);
        // 标志位：1 = 正控制着你；2 = 我这边有东西等你确认，请回一个包
        w.u8(self.active as u8 | (wants_ack as u8) << 1);
        w.i32(self.motion.0); w.i32(self.motion.1);
        w.i32(self.wheel.0); w.i32(self.wheel.1);
        w.u8(self.unacked.len() as u8);
        for (id, ev, at) in &self.unacked {
            w.u32(*id);
            w.i32(at.0); w.i32(at.1);
            match ev {
                Event::Key { usage, down } => { w.u8(1); w.u16(*usage); w.u8(*down as u8) }
                Event::Button { button, down } => { w.u8(2); w.u8(*button); w.u8(*down as u8) }
                Event::Enter { side, permille, mods, epoch } => { w.u8(3); w.u8(*side as u8); w.u16(*permille); w.u32(*mods); w.u32(*epoch) }
                Event::Leave => w.u8(4),
                Event::TakeBack { epoch } => { w.u8(5); w.u32(*epoch) }
                Event::Edge { side, permille, epoch } => { w.u8(6); w.u8(*side as u8); w.u16(*permille); w.u32(*epoch) }
            }
        }
        self.dirty = false; self.ack_dirty = false; self.last_send = Some(now_ms);
        Some(w.0)
    }

    /// 收到对方一个包（已解密）。返回该做的事。重复的、晚到的包不会让任何事做两遍。
    pub fn receive(&mut self, packet: &[u8]) -> Result<Vec<Action>, WireError> {
        let mut r = Reader { buf: packet, at: 0 };
        let seq = r.u64()?;
        let ack = r.u32()?;
        let ack_state = r.u64()?;
        let state_seq = r.u64()?;
        let flags = r.u8()?;
        if flags > 3 || state_seq > seq { return Err(WireError::BadValue) }
        let motion = (r.i32()?, r.i32()?);
        let wheel = (r.i32()?, r.i32()?);
        let n = r.u8()? as usize;
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let id = r.u32()?;
            let at = (r.i32()?, r.i32()?);
            let ev = match r.u8()? {
                1 => Event::Key { usage: r.u16()?, down: r.flag()? },
                2 => Event::Button { button: r.u8()?, down: r.flag()? },
                3 => {
                    let side = side_of(r.u8()?)?;
                    let permille = r.u16()?; if permille > 1000 { return Err(WireError::BadValue) }
                    Event::Enter { side, permille, mods: r.u32()?, epoch: r.u32()? }
                }
                4 => Event::Leave,
                5 => Event::TakeBack { epoch: r.u32()? },
                6 => {
                    let side = side_of(r.u8()?)?;
                    let permille = r.u16()?; if permille > 1000 { return Err(WireError::BadValue) }
                    Event::Edge { side, permille, epoch: r.u32()? }
                }
                t => return Err(WireError::BadType(t)),
            };
            events.push((id, ev, at));
        }
        if r.at != packet.len() { return Err(WireError::BadValue) }

        // 对方确认收到了哪些：这些以后不用再带
        while self.unacked.front().map_or(false, |(id, ..)| *id <= ack) { self.unacked.pop_front(); self.waiting_since = None }   // 有进展：重新计时
        if ack_state > self.acked_state { self.acked_state = ack_state.min(self.seq) }
        if state_seq > self.peer_state { self.peer_state = state_seq }
        if flags & 2 != 0 { self.ack_dirty = true }                             // 对方在等确认：回一个包

        let mut out = Vec::new();
        for (id, ev, at) in events {
            if id <= self.applied { self.ack_dirty = true; continue }           // 做过了：只需再确认一次，让对方别再带
            if id != self.applied + 1 { return Err(WireError::BadValue) }       // 中间缺了一个：对方不该这样发
            self.applied = id; self.ack_dirty = true;
            self.catch_up(at, &mut out);                                        // 先把鼠标走到事件发生时的地方
            out.push(Action::Event(ev));
        }
        if seq > self.peer_seq {                                                // 比上一个新，累计量才算数
            self.peer_seq = seq;
            let (dx, dy) = (wheel.0.wrapping_sub(self.peer_wheel.0), wheel.1.wrapping_sub(self.peer_wheel.1));
            self.peer_wheel = wheel;
            if dx != 0 || dy != 0 { out.push(Action::Wheel { dx, dy }) }
            self.catch_up(motion, &mut out);
        }
        Ok(out)
    }
    fn catch_up(&mut self, to: (i32, i32), out: &mut Vec<Action>) {
        let (dx, dy) = (to.0.wrapping_sub(self.peer_motion.0), to.1.wrapping_sub(self.peer_motion.1));
        self.peer_motion = to;
        if dx != 0 || dy != 0 { out.push(Action::Motion { dx, dy }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个会丢包、会乱序、会重复的假网络
    struct Net { seed: u64, loss: u32, in_flight: Vec<(u64, bool, Vec<u8>)> }
    impl Net {
        fn rnd(&mut self, n: u32) -> u32 { self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((self.seed >> 33) % n as u64) as u32 }
        /// 发一个包：按丢包率丢掉，否则 1–6 毫秒后到（所以会乱序），偶尔到两次
        fn send(&mut self, now: u64, to_b: bool, p: Vec<u8>) {
            if self.rnd(100) < self.loss { return }
            let at = now + 1 + self.rnd(6) as u64;
            if self.rnd(20) == 0 { self.in_flight.push((at + 2, to_b, p.clone())) }
            self.in_flight.push((at, to_b, p));
        }
        fn arrived(&mut self, now: u64) -> Vec<(bool, Vec<u8>)> {
            let mut out = Vec::new();
            let mut i = 0;
            while i < self.in_flight.len() { if self.in_flight[i].0 <= now { let x = self.in_flight.remove(i); out.push((x.1, x.2)) } else { i += 1 } }
            out
        }
    }

    /// 甲控制乙：甲不停地动鼠标、滚滚轮、按键、点击；看乙最后做出来的是不是一样。
    /// 返回（事件个数，最慢的一次晚到多少毫秒，甲发了多少个包，乙发了多少个包）
    fn run(loss: u32) -> (usize, u64, u32, u32) {
        let mut net = Net { seed: 20261112 + loss as u64, loss, in_flight: Vec::new() };
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        let mut sent: Vec<(Event, (i64, i64))> = Vec::new();        // 事件，和它发生时鼠标一共走了多少
        let mut got: Vec<(Event, (i64, i64))> = Vec::new();
        let (mut wheel_sent, mut wheel_got) = (0i64, 0i64);
        let (mut walked, mut b_walked) = ((0i64, 0i64), (0i64, 0i64));
        let mut sent_at: Vec<u64> = Vec::new();
        let (mut worst, mut from_a, mut from_b) = (0u64, 0u32, 0u32);
        a.hold(true);
        assert!(a.push(Event::Enter { side: Side::Left, permille: 500, mods: 0, epoch: 1 }));
        sent.push((Event::Enter { side: Side::Left, permille: 500, mods: 0, epoch: 1 }, walked)); sent_at.push(0);
        let end = 4000u64;
        for now in 0..end + 400 {
            if now < end {
                if now % 2 == 0 { let d = ((now % 7) as i32 - 3, (now % 5) as i32 - 2); a.motion(d.0, d.1); walked = (walked.0 + d.0 as i64, walked.1 + d.1 as i64) }
                if now % 50 == 3 { a.wheel(0, -3); wheel_sent -= 3 }
                if now % 37 == 5 {
                    let down = (now / 37) % 2 == 0;
                    let ev = if (now / 37) % 6 < 4 { Event::Key { usage: 4 + (now % 20) as u16, down } } else { Event::Button { button: 1, down } };
                    assert!(a.push(ev.clone()), "丢包 {loss}%：事件攒满了");
                    sent.push((ev, walked)); sent_at.push(now);
                }
            }
            if let Some(p) = a.poll(now) { from_a += 1; net.send(now, true, p) }
            if let Some(p) = b.poll(now) { from_b += 1; net.send(now, false, p) }
            for (to_b, p) in net.arrived(now) {
                if to_b {
                    for act in b.receive(&p).unwrap() {
                        match act {
                            Action::Event(e) => { worst = worst.max(now - sent_at[got.len()]); got.push((e, b_walked)) }
                            Action::Wheel { dy, .. } => wheel_got += dy as i64,
                            Action::Motion { dx, dy } => b_walked = (b_walked.0 + dx as i64, b_walked.1 + dy as i64),
                        }
                    }
                } else { a.receive(&p).unwrap(); }
            }
        }
        assert_eq!(got, sent, "丢包 {loss}%：按键和点击一个不少、顺序不乱、不重复，而且做的时候鼠标正好走到它发生时的地方");
        assert_eq!(wheel_got, wheel_sent, "丢包 {loss}%：滚轮总量必须一致");
        assert_eq!(b_walked, walked, "丢包 {loss}%：鼠标一共走的必须一样多");
        assert_eq!(a.pending(), 0, "丢包 {loss}%：最后应该全部确认完");
        (got.len(), worst, from_a, from_b)
    }

    #[test]
    fn 不丢包时一切照常() {
        let (n, worst, from_a, from_b) = run(0);
        assert!(n > 100);
        assert!(worst <= 7, "最慢 {worst} 毫秒");
        // 鼠标每 2 毫秒动一次，甲发了两千来个包；乙只在有按键点击、鼠标停下时回话，不逐包确认
        assert!(from_a > 1900 && from_b < from_a / 2, "甲发 {from_a} 个，乙回 {from_b} 个");
    }

    #[test]
    fn 丢包一成也不丢键不乱序() {
        for loss in [5, 10, 20] {
            let (_, worst, ..) = run(loss);
            // 假网络本身最多耽搁 6 毫秒；丢了靠补发，每 12 毫秒一次
            assert!(worst <= 100, "丢包 {loss}% 时最慢的一次按键晚了 {worst} 毫秒");
            println!("丢包 {loss}%：最慢的一次按键晚到 {worst} 毫秒");
        }
    }

    #[test]
    fn 丢包一半还是对的只是慢() { run(50); }

    #[test]
    fn 旧包晚到不会让鼠标多走() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        a.motion(10, 10); let old = a.poll(0).unwrap();
        a.motion(5, -4); let new = a.poll(8).unwrap();
        assert_eq!(b.receive(&new).unwrap(), vec![Action::Motion { dx: 15, dy: 6 }]);     // 中间那个包丢了也不少走
        assert_eq!(b.receive(&old).unwrap(), vec![]);                     // 晚到的旧包：不理
        assert_eq!(b.receive(&new).unwrap(), vec![]);                     // 同一个包到两次：也不理
    }

    #[test]
    fn 点击之前先把鼠标走到位() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        a.motion(30, 0);
        a.push(Event::Button { button: 1, down: true });
        a.motion(7, 0);                                                   // 点完又走了一点
        a.push(Event::Button { button: 1, down: false });
        a.motion(1, 1);
        let p = a.poll(0).unwrap();
        assert_eq!(b.receive(&p).unwrap(), vec![
            Action::Motion { dx: 30, dy: 0 }, Action::Event(Event::Button { button: 1, down: true }),
            Action::Motion { dx: 7, dy: 0 }, Action::Event(Event::Button { button: 1, down: false }),
            Action::Motion { dx: 1, dy: 1 },
        ]);
    }

    #[test]
    fn 没事的时候不来回空转() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        a.hold(true); a.motion(100, 100); a.push(Event::Key { usage: 4, down: true });
        let (mut from_a, mut from_b) = (0, 0);
        for now in 0..1000u64 {
            if let Some(p) = a.poll(now) { if now > 100 { from_a += 1 }; b.receive(&p).unwrap(); }
            if let Some(p) = b.poll(now) { if now > 100 { from_b += 1 }; a.receive(&p).unwrap(); }
        }
        assert_eq!(a.pending(), 0);
        assert!(from_a <= 4, "控制方静止时每 250 毫秒报一次平安就够了，实际发了 {from_a} 个");
        assert_eq!(from_b, 0, "被控方没事不该发包");
        a.hold(false);
        assert!(!a.active());
    }

    #[test]
    fn 鼠标停下后最后的移动量要确认() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        a.motion(3, 3);
        let _lost = a.poll(0).unwrap();                                   // 这个包丢了
        assert!(b.poll(0).is_none());
        assert!(a.poll(RESEND_MS + 5).is_none(), "还没到补发的时候（移动量比按键补得慢）");
        let again = a.poll(MOTION_RESEND_MS).expect("没确认：补发");
        assert_eq!(b.receive(&again).unwrap(), vec![Action::Motion { dx: 3, dy: 3 }]);
        a.receive(&b.poll(MOTION_RESEND_MS + 1).expect("补发的包要求确认")).unwrap();
        assert_eq!(a.pending(), 0);
        assert!(a.poll(MOTION_RESEND_MS * 3).is_none());
    }

    #[test]
    fn 鼠标慢慢动的时候也不逐包确认() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        let (mut from_a, mut from_b, mut walked) = (0, 0, 0);
        // 每 16 毫秒动一下（60 帧的屏幕、慢慢挪），一共 100 下；两边每 4 毫秒看一次要不要发
        for t in 0..1700u64 {
            if t % 16 == 0 && t < 1600 { a.motion(1, 0) }
            if t % 4 == 0 || t % 16 == 0 {
                if let Some(p) = a.poll(t) { from_a += 1; for act in b.receive(&p).unwrap() { if let Action::Motion { dx, .. } = act { walked += dx } } }
                if let Some(p) = b.poll(t) { from_b += 1; a.receive(&p).unwrap(); }
            }
        }
        assert_eq!(walked, 100);
        assert_eq!(a.pending(), 0, "停下以后最后的累计量确认了");
        assert!(from_a <= 104 && from_b <= 4, "一次移动一个包，不逐包确认：甲发了 {from_a} 个，乙回了 {from_b} 个");
    }

    #[test]
    fn 对方久不回应会攒满() {
        let mut a = Endpoint::new();
        assert!(a.push(Event::TakeBack { epoch: 7 }));
        let p = a.poll(0).unwrap();
        assert_eq!(Endpoint::new().receive(&p).unwrap(), vec![Action::Event(Event::TakeBack { epoch: 7 })]);
        for i in 1..MAX_UNACKED { assert!(a.push(Event::Key { usage: i as u16, down: true })) }
        assert!(!a.push(Event::Key { usage: 4, down: true }));
        // “走了”“回去吧”“到边了”还说得出去，对方照样按顺序收到
        assert!(a.push(Event::Leave) && a.push(Event::TakeBack { epoch: 8 }) && a.push(Event::Edge { side: Side::Top, permille: 250, epoch: 8 }));
        let got = Endpoint::new().receive(&a.poll(20).unwrap()).unwrap();
        assert_eq!(got.len(), MAX_UNACKED + 3);
        assert_eq!(got[MAX_UNACKED..], [Action::Event(Event::Leave), Action::Event(Event::TakeBack { epoch: 8 }), Action::Event(Event::Edge { side: Side::Top, permille: 250, epoch: 8 })]);
        for _ in 3..MAX_PARTING { assert!(a.push(Event::Leave)) }
        assert!(!a.push(Event::Leave));
    }

    #[test]
    fn 等确认等太久会报出来() {
        let (mut a, mut b) = (Endpoint::new(), Endpoint::new());
        a.motion(1, 1);
        a.poll(0);
        assert!(!a.take_stalled(5000), "只有鼠标移动没确认不算");
        a.push(Event::Key { usage: 4, down: true });
        let p = a.poll(6000).unwrap();
        assert!(!a.take_stalled(6000 + STALL_MS));
        assert!(a.take_stalled(6001 + STALL_MS));
        assert!(!a.take_stalled(6002 + STALL_MS), "报过一次就重新计时");
        b.receive(&p).unwrap();
        a.receive(&b.poll(8000).unwrap()).unwrap();                       // 对方确认了
        a.poll(9000);
        assert!(!a.take_stalled(99999));
    }

    #[test]
    fn 坏包不会崩() {
        let mut a = Endpoint::new();
        a.motion(1, 2); a.push(Event::Button { button: 1, down: true }); a.wheel(1, -1);
        a.push(Event::Edge { side: Side::Left, permille: 5, epoch: 1 });
        let p = a.poll(0).unwrap();
        for cut in 0..p.len() { assert!(Endpoint::new().receive(&p[..cut]).is_err()) }
        let mut extra = p.clone(); extra.push(0);
        assert!(Endpoint::new().receive(&extra).is_err());
    }
}
