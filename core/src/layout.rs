//! 几台电脑怎么摆：每台占一个格子，左右或上下挨着的两台之间，鼠标可以穿过去。

use crate::wire::{DeviceId, Side};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub rev: u64,
    cells: Vec<(DeviceId, i16, i16)>,
}

impl Layout {
    pub fn cells(&self) -> &[(DeviceId, i16, i16)] { &self.cells }

    pub fn position(&self, id: &DeviceId) -> Option<(i16, i16)> {
        self.cells.iter().find(|c| &c.0 == id).map(|c| (c.1, c.2))
    }
    fn at(&self, x: i16, y: i16) -> Option<DeviceId> {
        self.cells.iter().find(|c| c.1 == x && c.2 == y).map(|c| c.0)
    }

    /// 把一台电脑放到某个格子上。那个格子上已经有别的电脑，就和它对调（原来没位置的话，把对方挤到旁边最近的空格）。
    pub fn place(&mut self, id: DeviceId, x: i16, y: i16) {
        let (x, y) = (x.clamp(-LIMIT, LIMIT), y.clamp(-LIMIT, LIMIT));
        let old = self.position(&id);
        if old == Some((x, y)) { return }
        if let Some(other) = self.at(x, y) {
            let to = old.unwrap_or_else(|| self.free_near(x, y));
            if let Some(c) = self.cells.iter_mut().find(|c| c.0 == other) { c.1 = to.0; c.2 = to.1 }
        }
        match self.cells.iter_mut().find(|c| c.0 == id) {
            Some(c) => { c.1 = x; c.2 = y }
            None => self.cells.push((id, x, y)),
        }
        self.cells.sort();                                          // 总是排好序：两边比内容时才比得出同一个结果
        self.rev += 1;
    }

    pub fn remove(&mut self, id: &DeviceId) {
        let n = self.cells.len();
        self.cells.retain(|c| &c.0 != id);
        if self.cells.len() != n { self.rev += 1 }
    }

    /// 离 (x, y) 最近的空格子（先右、左、下、上，再往外一圈）
    pub fn free_near(&self, x: i16, y: i16) -> (i16, i16) {
        for r in 1..=8i16 {
            for (dx, dy) in [(r, 0), (-r, 0), (0, r), (0, -r), (r, r), (-r, r), (r, -r), (-r, -r)] {
                if self.at(x + dx, y + dy).is_none() { return (x + dx, y + dy) }
            }
        }
        (x + 9, y)
    }

    /// 从这台电脑的某条边出去，会到哪一台。y 往下为正。
    pub fn neighbor(&self, id: &DeviceId, side: Side) -> Option<DeviceId> {
        let (x, y) = self.position(id)?;
        match side {
            Side::Left => self.at(x - 1, y),
            Side::Right => self.at(x + 1, y),
            Side::Top => self.at(x, y - 1),
            Side::Bottom => self.at(x, y + 1),
        }
    }

    /// 没挨着任何一台的电脑：鼠标到不了，界面上要标出来提醒
    pub fn lonely(&self) -> Vec<DeviceId> {
        if self.cells.len() < 2 { return Vec::new() }
        self.cells.iter().map(|c| c.0)
            .filter(|id| [Side::Left, Side::Right, Side::Top, Side::Bottom].iter().all(|s| self.neighbor(id, *s).is_none()))
            .collect()
    }

    /// 收到别的电脑发来的摆法：比自己的新就采用。返回有没有变。
    /// 一样新（两台各自摆了同样多次）时按内容比大小，两边都采用“大”的那份，这样总能一致。
    pub fn merge(&mut self, rev: u64, cells: &[(DeviceId, i16, i16)]) -> bool {
        if rev < self.rev || !valid(cells) { return false }
        let mut theirs = cells.to_vec(); theirs.sort();
        if rev == self.rev {
            let mut mine = self.cells.clone(); mine.sort();
            if theirs <= mine { return false }
        }
        self.rev = rev; self.cells = theirs;
        true
    }
}

/// 格子坐标的范围：别人发来的超出这个范围就不要，自己算邻居时也就不会越界
const LIMIT: i16 = 1000;
fn valid(cells: &[(DeviceId, i16, i16)]) -> bool {
    if cells.len() > 64 { return false }
    cells.iter().enumerate().all(|(i, c)| c.1.abs() <= LIMIT && c.2.abs() <= LIMIT
        && cells[..i].iter().all(|d| d.0 != c.0 && (d.1, d.2) != (c.1, c.2)))
}

/// 鼠标在一条边上的位置，换算成千分比（两台屏幕大小不一样，按比例对过去）
pub fn to_permille(pos: f64, length: f64) -> u16 {
    if length <= 1.0 { return 500 }
    ((pos / (length - 1.0)).clamp(0.0, 1.0) * 1000.0).round() as u16
}
/// 千分比换回这台屏幕上的位置
pub fn from_permille(permille: u16, length: f64) -> f64 {
    (permille.min(1000) as f64 / 1000.0) * (length - 1.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    const MAC: DeviceId = [1; 16];
    const WIN: DeviceId = [2; 16];
    const LINUX: DeviceId = [3; 16];

    #[test]
    fn 挨着的两台互为邻居() {
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(WIN, -1, 0); l.place(LINUX, 0, 1);
        assert_eq!(l.neighbor(&MAC, Side::Left), Some(WIN));
        assert_eq!(l.neighbor(&WIN, Side::Right), Some(MAC));
        assert_eq!(l.neighbor(&MAC, Side::Bottom), Some(LINUX));
        assert_eq!(l.neighbor(&LINUX, Side::Top), Some(MAC));
        assert_eq!(l.neighbor(&MAC, Side::Right), None);
        assert_eq!(l.neighbor(&WIN, Side::Bottom), None);          // 斜对角不算挨着
        assert!(l.lonely().is_empty());
    }

    #[test]
    fn 拖到别人身上就对调() {
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(WIN, 1, 0);
        l.place(WIN, 0, 0);
        assert_eq!(l.position(&WIN), Some((0, 0)));
        assert_eq!(l.position(&MAC), Some((1, 0)));
        l.place(LINUX, 0, 0);                                      // 新来的占了别人的位置：把原来那台挤到旁边空格
        assert_eq!(l.position(&LINUX), Some((0, 0)));
        assert_eq!(l.position(&WIN), Some((-1, 0)));
        assert_eq!(l.cells().len(), 3);
    }

    #[test]
    fn 落单的会被找出来() {
        let mut l = Layout::default();
        l.place(MAC, 0, 0); l.place(WIN, 1, 0); l.place(LINUX, 3, 2);
        assert_eq!(l.lonely(), vec![LINUX]);
    }

    #[test]
    fn 只采用更新的摆法() {
        let mut a = Layout::default();
        a.place(MAC, 0, 0); a.place(WIN, 1, 0);
        let mut b = Layout::default();
        assert!(b.merge(a.rev, a.cells()));
        assert_eq!(a, b);
        assert!(!b.merge(a.rev, &[]));                             // 同样新、内容“小”的不动
        assert_eq!(b.cells().len(), 2);
        assert!(!b.merge(a.rev - 1, &[(LINUX, 0, 0)]));            // 旧的不动
    }

    #[test]
    fn 一样新的两份摆法两边最后一致() {
        let (mut a, mut b) = (Layout::default(), Layout::default());
        a.place(MAC, 0, 0); a.place(WIN, 1, 0);
        b.place(WIN, 0, 0); b.place(MAC, 1, 0);
        let (sa, sb) = (a.clone(), b.clone());
        a.merge(sb.rev, sb.cells()); b.merge(sa.rev, sa.cells());
        assert_eq!(a, b);
    }

    #[test]
    fn 不合规矩的摆法不要() {
        let mut l = Layout::default();
        assert!(!l.merge(5, &[(MAC, i16::MAX, 0)]));               // 坐标太远
        assert!(!l.merge(5, &[(MAC, 0, 0), (WIN, 0, 0)]));         // 两台叠在一格
        assert!(!l.merge(5, &[(MAC, 0, 0), (MAC, 1, 0)]));         // 一台占两格
        l.place(MAC, i16::MAX, i16::MIN);
        assert_eq!(l.position(&MAC), Some((1000, -1000)));
        assert_eq!(l.neighbor(&MAC, Side::Right), None);
    }

    #[test]
    fn 大小不同的屏幕按比例对过去() {
        assert_eq!(to_permille(0.0, 1080.0), 0);
        assert_eq!(to_permille(1079.0, 1080.0), 1000);
        assert_eq!(to_permille(539.5, 1080.0), 500);
        assert_eq!(from_permille(1000, 1440.0), 1439.0);
        assert_eq!(from_permille(0, 1440.0), 0.0);
        let back = from_permille(to_permille(300.0, 1080.0), 1080.0);
        assert!((back - 300.0).abs() < 1.0);
    }
}
