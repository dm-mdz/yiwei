//! 剪贴板跟着指针走。规则（甲是拿着键鼠的那台，乙是指针进去的那台）：
//!   1. 甲的指针进到乙：乙的剪贴板临时换成甲的那份；
//!   2. 甲的指针在乙上时复制的东西：指针回到甲、或者直接去丙，跟着走；
//!   3. 甲的指针离开乙：乙的剪贴板恢复成甲来之前的样子；
//!   4. 甲在乙上什么都没复制：回来后甲的剪贴板不变。
//! 这里只管规则：哪一份什么时候放进本机的剪贴板、什么时候带给别的电脑。读写剪贴板、往外发由外壳实现 Board。
//! Windows 外壳用的是这一份；Mac 外壳用 Swift 另写了同样的一套（Share.swift 的“剪贴板跟着指针走”一节），改规则时两处一起改。

use crate::wire::ClipTag;
use std::collections::HashMap;

/// 外壳替规则做的事：读写本机的剪贴板、把内容带给别的电脑
pub trait Board {
    /// 整份剪贴板（所有格式）
    type Snap: Clone + PartialEq;
    /// 剪贴板现在是第几版（内容每变一次就不一样）
    fn version(&mut self) -> u32;
    /// 整份存下来。存不了（太大、打不开）返回 None
    fn snapshot(&mut self) -> Option<Self::Snap>;
    /// 整份放进剪贴板，放成了返回 true。guest：这是别的电脑的指针带来的、临时用的（别让它进系统的剪贴板历史）
    fn apply(&mut self, snap: &Self::Snap, guest: bool) -> bool;
    /// 把剪贴板里现在能带走的（文字、图片）带给 id 这台电脑，都附上 tag。什么都带不了（空的，或者复制的是文件）返回 false
    fn carry(&mut self, id: &str, tag: ClipTag) -> bool;
    /// 告诉 id：本机这次没有能带的，上次带去的那份别用了
    fn carry_nothing(&mut self, id: &str, tag: ClipTag);
    /// 两份并成一份（同一次带来的文字和图片）
    fn join(&mut self, a: &Self::Snap, b: &Self::Snap) -> Self::Snap;
    /// 往运行记录里写一句（内容不记，只记发生了什么）
    fn note(&mut self, _line: String) {}
}

pub struct Flow<S> {
    /// 客人进来之前本机自己的剪贴板（它走了放回去），以及这一份是剪贴板第几版时存的：
    /// 下一位客人进来时版本没变就接着用，不用把所有格式再读一遍（读一遍会逼着复制的那个程序把内容现做出来，大的会卡）
    own: Option<S>,
    own_at: u32,
    /// 每台电脑带到本机来的那份：它的指针在本机上时才放进剪贴板
    guests: HashMap<String, S>,
    /// 核心说“这台的指针要进来了”
    next_guest: Option<String>,
    /// 指针正在本机上的那台
    guest: Option<String>,
    /// 剪贴板里现在放的是客人的那份
    guest_has: bool,
    /// 客人进来（或者它的那份放上来）之后剪贴板是第几版。之后变了，多半是它在这儿复制了新的
    applied: u32,
    /// 本机的指针上次带给这台电脑的是剪贴板的第几版
    sent: HashMap<String, u32>,
    /// 本机的指针正在哪台电脑上
    remote: Option<String>,
    /// 本机往外带了几次（每带一次加一，附在带的内容上）
    carry_no: u32,
    /// 每台电脑、每个方向（是不是带回来的）最近收到的是它第几次带的；那一次的文字留着，同一次的图片到了并成一份
    seen: HashMap<(String, bool), (u32, Option<S>)>,
}
impl<S> Default for Flow<S> {
    fn default() -> Self {
        Flow { own: None, own_at: 0, guests: HashMap::new(), next_guest: None, guest: None, guest_has: false, applied: 0, sent: HashMap::new(), remote: None, carry_no: 0, seen: HashMap::new() }
    }
}

fn short(id: &str) -> &str { id.get(..6).unwrap_or(id) }

impl<S: Clone + PartialEq> Flow<S> {
    /// 本机的指针进到 id 这台电脑：把剪贴板带过去
    pub fn grabbed<B: Board<Snap = S>>(&mut self, b: &mut B, id: &str) { self.remote = Some(id.to_string()); self.bring(b, id) }
    /// 本机的指针回来了
    pub fn released(&mut self) { self.remote = None }
    fn next_no(&mut self) -> u32 { self.carry_no += 1; self.carry_no }
    /// 和上次带给它的是同一版就不带（它那边留着上次那份）
    fn bring<B: Board<Snap = S>>(&mut self, b: &mut B, id: &str) {
        if self.sent.get(id) == Some(&b.version()) { return }
        let tag = ClipTag { back: false, no: self.next_no() };
        let any = b.carry(id, tag);
        if !any { b.carry_nothing(id, tag) }
        self.sent.insert(id.to_string(), b.version());                        // 带完再记：Windows 上取“用的时候才生成”的内容，版本会跟着变一下
        b.note(format!("指针到了 {}：剪贴板{}", short(id), if any { "带过去" } else { "里没有能带的" }));
    }

    /// 核心说 id 的指针要进来了（紧接着是“进来了”）
    pub fn guest_coming(&mut self, id: &str) { self.next_guest = Some(id.to_string()) }
    /// 别的电脑的指针进来了：记下本机自己的剪贴板，换上它的那份
    pub fn guest_entered<B: Board<Snap = S>>(&mut self, b: &mut B) {
        self.guest = self.next_guest.clone();
        if self.own.is_none() || self.own_at != b.version() { self.own = b.snapshot(); self.own_at = b.version() }
        self.guest_has = false;
        self.applied = b.version();
        if let Some((g, s)) = self.guest.clone().and_then(|g| self.guests.get(&g).cloned().map(|s| (g, s))) { self.show(b, &g, s) }
        b.note(format!("{} 的指针进来了：剪贴板{}{}", short(self.guest.as_deref().unwrap_or("?")), if self.guest_has { "换成它的那份" } else { "先不动" }, if self.own.is_none() { "（本机的那份存不下）" } else { "" }));
    }
    /// 把客人的那份放进剪贴板，并把放好之后的样子记成它的那份：离开时拿来比，一样就是它在这儿没复制过
    fn show<B: Board<Snap = S>>(&mut self, b: &mut B, id: &str, snap: S) {
        if !b.apply(&snap, true) { return }
        self.guests.insert(id.to_string(), b.snapshot().unwrap_or(snap));
        self.guest_has = true; self.applied = b.version();
    }
    /// 放回本机自己的那份
    fn restore<B: Board<Snap = S>>(&mut self, b: &mut B) {
        if let Some(own) = &self.own { if b.apply(own, false) { self.own_at = b.version() } }
    }
    /// 它的指针走了。how：0 正常走了、1 被请回去了、2 掉线了。
    /// 它在这儿复制过东西就带回给它；本机的剪贴板恢复成它来之前的样子
    pub fn guest_left<B: Board<Snap = S>>(&mut self, b: &mut B, how: i32) {
        if let Some(g) = self.guest.take() {
            let changed = b.version() != self.applied;
            let now = if changed { b.snapshot() } else { None };
            // 版本变了不一定是复制过（别的程序来取“用的时候才生成”的内容、剪贴板工具改写，版本也会变）：内容和原来不一样才算。
            // 这会儿读不出来（被占着、太大）就当没复制过，它带来的那份照旧留着
            let copied = now.is_some() && now.as_ref() != if self.guest_has { self.guests.get(&g) } else { self.own.as_ref() };
            if copied {
                if how != 2 { let tag = ClipTag { back: true, no: self.next_no() }; b.carry(&g, tag); }
                // 剪贴板里现在是它新复制的：记成它的那份（它下次进来接着用，不用重新带）
                match now.filter(|_| how != 2) { Some(s) => { self.guests.insert(g.clone(), s); } None => { self.guests.remove(&g); } }
            }
            let dirty = changed || self.guest_has;
            if dirty { self.restore(b) }
            b.note(format!("{} 的指针走了（{how}）：{}，本机的剪贴板{}", short(&g),
                if !copied { "它在这儿没复制过" } else if how == 2 { "它在这儿复制过，掉线了带不回去" } else { "它在这儿复制过，带回给它" },
                if !dirty { "没动过" } else if self.own.is_some() { "恢复原样" } else { "恢复不了" }));
        }
        self.guest_has = false;
        self.applied = b.version();
    }

    /// id 带来了一份剪贴板内容。tag.back：本机的指针在它上面时复制的，跟着指针回来了；不然是它的指针带到本机来的。
    /// image：这一份是图片（同一次带的，文字先到、图片后到）
    pub fn arrived<B: Board<Snap = S>>(&mut self, b: &mut B, id: &str, tag: ClipTag, image: bool, piece: S) {
        let key = (id.to_string(), tag.back);
        let (last, text) = self.seen.get(&key).map_or((0, None), |s| (s.0, s.1.clone()));
        if tag.no < last { return }                                           // 比已经收到的还早的一次：晚到了，不要
        let snap = match text { Some(t) if tag.no == last && image => b.join(&t, &piece), _ => piece };
        self.seen.insert(key, (tag.no, if image { None } else { Some(snap.clone()) }));
        if tag.back {
            // 这会儿本机上有客人的话，它算“本机自己的那份”：客人走了，剪贴板里就是它。客人没往剪贴板里放过东西就现在换上
            if self.guest.is_none() { b.apply(&snap, false); }
            else {
                if !self.guest_has && b.version() == self.applied && b.apply(&snap, false) { self.applied = b.version(); self.own_at = self.applied }
                self.own = Some(snap);
            }
            self.sent.insert(id.to_string(), b.version());                    // 它那边留着这一份，下次过去不用再带
            if let Some(r) = self.remote.clone().filter(|r| r != id) { self.bring(b, &r) }   // 指针已经到了另一台：接着带过去
        } else if self.guest.as_deref() != Some(id) {
            self.guests.insert(id.to_string(), snap);                         // 它的指针不在本机上：先记着（带来的可能比“进来了”早到一步）
        } else if b.version() == self.applied {
            self.show(b, id, snap);
        }                                                                     // 不然是它刚在这儿复制了新的：带来的那份更早，不拿它盖掉
    }
    /// id 说它这次（第 no 次带）没有能带的（比如它复制的是文件）：它上次带来的那份不要了
    pub fn cleared<B: Board<Snap = S>>(&mut self, b: &mut B, id: &str, no: u32) {
        let key = (id.to_string(), false);
        if self.seen.get(&key).map_or(false, |s| no < s.0) { return }
        self.seen.insert(key, (no, None));
        self.guests.remove(id);
        if self.guest.as_deref() != Some(id) || !self.guest_has || b.version() != self.applied { return }
        self.restore(b);
        self.guest_has = false; self.applied = b.version();
    }
    /// 带给 id 的图片、文件半路没带成：别当成“带过了”，下次指针过去重新带
    pub fn forget_sent(&mut self, id: &str) { self.sent.remove(id); }
    /// 一台电脑断开了：它重新连上以后，剪贴板从头带（它重开以后“第几次带”也从头数）
    pub fn disconnected(&mut self, id: &str) { self.sent.remove(id); self.guests.remove(id); self.seen.retain(|k, _| k.0 != id) }
    /// 键鼠共享关掉了：客人还在、剪贴板被它动过，就恢复成本机自己的；其余都忘掉
    pub fn reset<B: Board<Snap = S>>(&mut self, b: &mut B) {
        if self.guest.is_some() && (self.guest_has || b.version() != self.applied) { self.restore(b) }
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假的剪贴板：内容就是一个字符串；“文件:”开头的算带不走的
    #[derive(Default)]
    struct Fake { text: String, version: u32, carried: Vec<(String, bool, String)>, nothing: Vec<String>, too_big: bool, busy: bool, reads: u32, guest_marks: Vec<bool> }
    impl Fake {
        fn copy(&mut self, s: &str) { self.text = s.into(); self.version += 1 }
        fn take(&mut self) -> Vec<(String, bool, String)> { std::mem::take(&mut self.carried) }
    }
    impl Board for Fake {
        type Snap = String;
        fn version(&mut self) -> u32 { self.version }
        fn snapshot(&mut self) -> Option<String> { self.reads += 1; if self.too_big || self.busy { None } else { Some(self.text.clone()) } }
        fn apply(&mut self, s: &String, guest: bool) -> bool { if self.busy { return false } self.guest_marks.push(guest); self.copy(&s.clone()); true }
        fn carry(&mut self, id: &str, tag: ClipTag) -> bool {
            if self.text.is_empty() || self.text.starts_with("文件:") { return false }
            self.carried.push((id.into(), tag.back, self.text.clone())); true
        }
        fn carry_nothing(&mut self, id: &str, _: ClipTag) { self.nothing.push(id.into()) }
        fn join(&mut self, a: &String, b: &String) -> String { format!("{a}+{b}") }
    }
    fn enter(f: &mut Flow<String>, b: &mut Fake, id: &str) { f.guest_coming(id); f.guest_entered(b) }
    fn sent(id: &str, back: bool, s: &str) -> (String, bool, String) { (id.into(), back, s.into()) }
    fn came(no: u32) -> ClipTag { ClipTag { back: false, no } }
    fn back(no: u32) -> ClipTag { ClipTag { back: true, no } }

    #[test]
    fn 本机当乙_客人的那份只在它的指针在时用() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("乙自己的");
        f.arrived(&mut b, "A", came(1), false, "甲的".into());
        assert_eq!(b.text, "乙自己的", "带来的比“进来了”早到一步：先记着，不动本机的");
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲的", "规则 1：甲的指针进来，换成甲的那份");
        assert_eq!(b.guest_marks, vec![true], "客人的那份要标出来，不进系统的剪贴板历史");
        f.guest_left(&mut b, 0);
        assert_eq!(b.text, "乙自己的", "规则 3：甲的指针走了，恢复成原来的");
        assert_eq!(b.guest_marks, vec![true, false]);
        assert_eq!(b.take(), vec![], "规则 4：甲在这儿没复制过，什么都不往回带");
        let reads = b.reads;
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲的", "甲没复制新的，再进来还是那份");
        assert_eq!(b.reads, reads + 1, "本机的那份没变过：不重新读一遍（只读了放上去的客人那份）");
        b.copy("甲在乙上复制的");
        f.guest_left(&mut b, 0);
        assert_eq!(b.take(), vec![sent("A", true, "甲在乙上复制的")], "规则 2：在乙上复制的带回给甲");
        assert_eq!(b.text, "乙自己的");
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲在乙上复制的", "甲再进来：是它上次在乙上复制的");
        f.arrived(&mut b, "A", came(2), false, "字".into());
        f.arrived(&mut b, "A", came(2), true, "图".into());
        assert_eq!(b.text, "字+图", "同一次带来的文字和图片并成一份");
        f.arrived(&mut b, "A", came(3), true, "另一张图".into());
        assert_eq!(b.text, "另一张图", "另一次带来的图片不和上次的文字并");
        f.arrived(&mut b, "A", came(2), true, "晚到的旧图".into());
        assert_eq!(b.text, "另一张图", "更早那次带的晚到了：不要");
        f.cleared(&mut b, "A", 4);
        assert_eq!(b.text, "乙自己的", "甲说这次没有能带的：换回本机的");
        f.arrived(&mut b, "A", came(3), true, "又一张晚到的旧图".into());
        assert_eq!(b.text, "乙自己的", "“没有能带的”之后晚到的旧图：不要");
        f.guest_left(&mut b, 0);
        assert_eq!((b.take(), b.text.as_str()), (vec![], "乙自己的"));
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "乙自己的", "甲上次带来的那份已经不要了");
        f.guest_left(&mut b, 0);
        // 没带东西的客人来了又走、客人不在时带来的：本机的都不变
        enter(&mut f, &mut b, "D"); f.guest_left(&mut b, 0);
        f.arrived(&mut b, "A", came(5), false, "甲不在时带来的".into());
        assert_eq!((b.take(), b.text.as_str()), (vec![], "乙自己的"));
    }

    #[test]
    fn 版本变了内容没变_不算复制过() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("乙自己的");
        // 客人没带东西：版本变了、内容没变
        enter(&mut f, &mut b, "D"); b.version += 1; f.guest_left(&mut b, 0);
        assert_eq!((b.take(), b.text.as_str()), (vec![], "乙自己的"));
        // 客人的那份在剪贴板上：版本变了、内容没变，不能把它带来的再“带回去”盖掉它原来的（规则 4）
        f.arrived(&mut b, "A", came(1), false, "甲的".into());
        enter(&mut f, &mut b, "A"); b.version += 1; f.guest_left(&mut b, 0);
        assert_eq!((b.take(), b.text.as_str()), (vec![], "乙自己的"));
        // 离开时剪贴板读不出来：当没复制过，它带来的那份照旧留着
        enter(&mut f, &mut b, "A"); b.version += 1; b.busy = true; f.guest_left(&mut b, 0); b.busy = false;
        assert_eq!(b.take(), vec![]);
        b.copy("乙自己的");                                                    // 刚才占着没恢复成，这里手动摆回去
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲的", "规则 1：再进来还是甲的那份");
        f.guest_left(&mut b, 0);
        // 进来时剪贴板打不开、没换成：离开时不能把乙自己的内容记成“甲的那份”
        b.busy = true; enter(&mut f, &mut b, "A"); b.busy = false;
        f.guest_left(&mut b, 0);
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲的");
    }

    #[test]
    fn 客人刚复制了新的_晚到的旧内容不盖掉它() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("乙自己的");
        enter(&mut f, &mut b, "A");
        f.arrived(&mut b, "A", came(1), false, "甲带来的字".into());
        b.copy("甲刚在乙上复制的");
        f.arrived(&mut b, "A", came(1), true, "甲带来的大图，晚到了".into());
        assert_eq!(b.text, "甲刚在乙上复制的");
        f.cleared(&mut b, "A", 2);
        assert_eq!(b.text, "甲刚在乙上复制的");
        f.guest_left(&mut b, 0);
        assert_eq!((b.take(), b.text.as_str()), (vec![sent("A", true, "甲刚在乙上复制的")], "乙自己的"), "规则 2：新复制的照样带回去");
    }

    #[test]
    fn 客人被请回去也带回给它_掉线了就不带() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("乙自己的");
        enter(&mut f, &mut b, "A"); b.copy("甲复制的一");
        // 丙的指针进来把甲请回去：先说“丙要进来”，再是“甲走了”“丙进来了”。甲复制的还是带回给甲，不能记到丙名下
        f.guest_coming("C"); f.guest_left(&mut b, 1); f.guest_entered(&mut b);
        assert_eq!(b.take(), vec![sent("A", true, "甲复制的一")]);
        assert_eq!(b.text, "乙自己的", "丙没带东西来：剪贴板是乙自己的");
        b.copy("丙复制的");
        f.guest_left(&mut b, 2);
        assert_eq!((b.take(), b.text.as_str()), (vec![], "乙自己的"), "丙掉线了：带不回去，本机照样恢复");
        f.disconnected("C");
        enter(&mut f, &mut b, "C");
        assert_eq!(b.text, "乙自己的", "掉线的客人留下的那份不留着");
        f.arrived(&mut b, "C", came(1), false, "丙重新连上后带来的".into());
        assert_eq!(b.text, "丙重新连上后带来的", "它重开以后“第几次带”从头数，照样收");
        f.guest_left(&mut b, 0);
        // 本机的那份存不下（太大）：客人走了恢复不了，但别的照常
        b.copy("乙自己的很大的一份"); b.too_big = true;
        f.arrived(&mut b, "A", came(9), false, "甲的".into());
        enter(&mut f, &mut b, "A"); f.guest_left(&mut b, 0);
        assert_eq!(b.text, "甲的");
    }

    #[test]
    fn 本机当甲_变了才带_带回来的接着带去下一台() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("甲的一");
        f.grabbed(&mut b, "B");
        assert_eq!(b.take(), vec![sent("B", false, "甲的一")]);
        f.released(); f.grabbed(&mut b, "B");
        assert_eq!(b.take(), vec![], "没复制新的：不重复带");
        f.released();
        b.copy("文件:a.txt");
        f.grabbed(&mut b, "B");
        assert_eq!((b.take(), std::mem::take(&mut b.nothing)), (vec![], vec!["B".to_string()]), "复制的是文件：告诉对方这次没有能带的");
        // 在乙上复制的跟着指针回来
        f.released();
        f.arrived(&mut b, "B", back(1), false, "在乙上复制的".into());
        assert_eq!(b.text, "在乙上复制的");
        assert_eq!(b.guest_marks, vec![false], "带回来的是本机自己的了，不用标");
        f.grabbed(&mut b, "B");
        assert_eq!(b.take(), vec![], "乙那边留着这一份：再过去不用带");
        // 从乙直接去丙：先带本机现有的，乙那边带回来的到了再接着带给丙
        b.copy("甲的二");
        f.grabbed(&mut b, "B"); b.take();
        f.grabbed(&mut b, "C");
        assert_eq!(b.take(), vec![sent("C", false, "甲的二")]);
        f.arrived(&mut b, "B", back(2), false, "又在乙上复制的".into());
        assert_eq!(b.take(), vec![sent("C", false, "又在乙上复制的")]);
        // 半路没带成的：下次过去重新带
        f.released(); f.forget_sent("C"); f.grabbed(&mut b, "C");
        assert_eq!(b.take(), vec![sent("C", false, "又在乙上复制的")]);
        // 断开过的电脑：重新连上以后从头带
        f.released(); f.disconnected("C");
        f.grabbed(&mut b, "C");
        assert_eq!(b.take(), vec![sent("C", false, "又在乙上复制的")]);
    }

    #[test]
    fn 带回来的时候本机上有客人_算本机自己的那份() {
        let (mut f, mut b) = (Flow::default(), Fake::default());
        b.copy("本机原来的");
        enter(&mut f, &mut b, "A");
        f.arrived(&mut b, "C", back(1), false, "在丙上复制的".into());
        assert_eq!(b.text, "在丙上复制的", "客人没往剪贴板里放过东西：现在就换上");
        f.arrived(&mut b, "A", came(1), false, "甲带来的".into());
        assert_eq!(b.text, "甲带来的");
        f.arrived(&mut b, "C", back(2), false, "又在丙上复制的".into());
        assert_eq!(b.text, "甲带来的", "客人的那份正在用：不打断，记成本机自己的");
        f.guest_left(&mut b, 0);
        assert_eq!((b.take(), b.text.as_str()), (vec![], "又在丙上复制的"));
        // 关掉键鼠共享时客人的那份还在：恢复成本机自己的
        enter(&mut f, &mut b, "A");
        assert_eq!(b.text, "甲带来的");
        f.reset(&mut b);
        assert_eq!(b.text, "又在丙上复制的");
        // 关掉时客人没带东西、但在这儿复制过：也恢复
        enter(&mut f, &mut b, "D"); b.copy("丁复制的");
        f.reset(&mut b);
        assert_eq!(b.text, "又在丙上复制的");
    }
}
