//! 两台电脑之间的消息，以及它们在网络上的样子。
//!
//! 每条消息是一帧：4 字节长度（大端，不含自己）+ 1 字节类型 + 内容。
//! 数字一律大端；字符串是 2 字节长度 + UTF-8。加密在这一层外面做，这里只管明文的编码和解码。

pub const PROTOCOL: u16 = 13;
/// 一帧最大 16 MB（剪贴板里的图片会比较大），超过就当成坏数据
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os { Mac = 1, Windows = 2, Linux = 3 }

/// 屏幕的四条边
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side { Left = 0, Right = 1, Top = 2, Bottom = 3 }

impl Side {
    pub fn opposite(self) -> Side {
        match self { Side::Left => Side::Right, Side::Right => Side::Left, Side::Top => Side::Bottom, Side::Bottom => Side::Top }
    }
}

pub type DeviceId = [u8; 16];

/// 带剪贴板内容时附的记号（剪贴板跟着指针走，规则见 clipflow）。
/// back = false：我的指针进到你那儿，这是我带来的，只在我的指针在你那儿时用；back = true：你的指针在我这儿时复制的，现在跟着你的指针回去。
/// no：我这边第几次往外带（每带一次加一）。同一次带的文字和图片 no 相同；比已经收到的更早的一次晚到了就不要
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClipTag { pub back: bool, pub no: u32 }
/// 走发文件那条路带的剪贴板内容是哪一种：一张图片，还是复制的文件、文件夹
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind { Image, Files }

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// 连上之后先自我介绍
    /// port 是我收鼠标键盘（UDP）用的端口；proof：我是哪一组的证明（用组钥匙和这次握手算出来的，同组的才对得上）
    Hello { protocol: u16, id: DeviceId, os: Os, name: String, width: u32, height: u32, port: u16, proof: [u8; 32] },
    /// 我的屏幕大小变了
    Size { width: u32, height: u32 },
    /// 发文件：先报一次要发哪些（相对路径用 / 分隔、大小），然后按顺序一块一块发内容，最后说一声发完了
    /// clip 有值：这不是用户点“发送文件”发的，是剪贴板里的图片（只有一个文件）或者复制的文件、文件夹：
    /// 收下后交给剪贴板，不提示、不放进“收到的文件”
    FileBegin { xfer: u32, files: Vec<(String, u64)>, clip: Option<(ClipTag, ClipKind)> },
    FileData { xfer: u32, data: Vec<u8> },
    FileEnd { xfer: u32 },
    /// 发的一方说：不发了（xfer 是它自己的编号）
    FileCancel { xfer: u32 },
    /// 收的一方回话（xfer 是发的一方的编号）：收好了（ok），或者不收了、出错了（err 说原因）
    FileAck { xfer: u32, ok: bool, err: String },
    /// 几台电脑的摆法。rev 越大越新，收到更新的就照它来
    Layout { rev: u64, cells: Vec<(DeviceId, i16, i16)> },
    /// 鼠标从我这边穿到你那边了：从你的哪条边进来、在那条边的千分之几处、此刻按着哪些修饰键
    Enter { side: Side, permille: u16, mods: u32 },
    /// 鼠标回到我这边了，你那边松开所有按键
    Leave,
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: u8, down: bool },
    Wheel { dx: i32, dy: i32 },
    /// 按键用 USB 键盘标准里的键位编号，三个系统各自再换成自己的
    Key { usage: u16, down: bool },
    /// 剪贴板里的文字（切到对面时带过去）
    /// 剪贴板里的文字。不是带回去的话，空的表示“我这次没有能带的”
    ClipText { text: String, tag: ClipTag },
    Ping { nonce: u32 },
    Pong { nonce: u32 },
    /// 加入一组（见 secure 模块的 `Join`）：想加入的一方先开口，被找的一方回一段，想加入的一方再证明自己知道校验码
    JoinAsk { msg: Vec<u8> },
    JoinReply { msg: Vec<u8> },
    JoinProve { mac: [u8; 32] },
    /// 被找的那台的用户点了“允许”：证明我也知道校验码，并把这一组的校验码、组钥匙、第几版交给新成员
    JoinGrant { mac: [u8; 32], code: String, key: [u8; 32], rev: u64 },
    /// 不行：1 校验码不对、2 用户没同意、3 暂时不接受加入（输错太多次，或者等着答复的太多）
    JoinDeny { why: u8 },
    /// 这一组换了校验码和组钥匙（有人点了“换一个”，或者请出了一台）：组员跟着换
    /// 组里别的电脑在哪（编号、IPv4 地址、端口）：收到的一方要是还没连上它们，就自己去连。
    /// 有的电脑收不到别人的广播（装了代理软件、虚拟网卡），靠组员互相转告也能连成一片
    Members { list: Vec<(DeviceId, [u8; 4], u16)> },
    /// gone：被请出去的那台（有的话），大家都和它断开
    Regroup { code: String, key: [u8; 32], rev: u64, gone: Option<DeviceId> },
}

#[derive(Debug, PartialEq)]
pub enum WireError { Short, TooLong, BadType(u8), BadValue }

pub(crate) struct Writer(pub(crate) Vec<u8>);
impl Writer {
    pub(crate) fn u8(&mut self, v: u8) { self.0.push(v) }
    pub(crate) fn u16(&mut self, v: u16) { self.0.extend_from_slice(&v.to_be_bytes()) }
    pub(crate) fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_be_bytes()) }
    pub(crate) fn u64(&mut self, v: u64) { self.0.extend_from_slice(&v.to_be_bytes()) }
    pub(crate) fn i16(&mut self, v: i16) { self.0.extend_from_slice(&v.to_be_bytes()) }
    pub(crate) fn i32(&mut self, v: i32) { self.0.extend_from_slice(&v.to_be_bytes()) }
    pub(crate) fn bytes(&mut self, v: &[u8]) { self.0.extend_from_slice(v) }
    pub(crate) fn short_str_bytes(&mut self, v: &[u8]) { let b = &v[..v.len().min(255)]; self.u8(b.len() as u8); self.bytes(b) }
    pub(crate) fn short_str(&mut self, v: &str) { let b = &v.as_bytes()[..v.len().min(u16::MAX as usize)]; self.u16(b.len() as u16); self.bytes(b) }
    pub(crate) fn long_str(&mut self, v: &str) { self.u32(v.len() as u32); self.bytes(v.as_bytes()) }
}

pub(crate) struct Reader<'a> { pub(crate) buf: &'a [u8], pub(crate) at: usize }
impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.buf.len() - self.at < n { return Err(WireError::Short) }
        let s = &self.buf[self.at..self.at + n]; self.at += n; Ok(s)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, WireError> { Ok(self.take(1)?[0]) }
    pub(crate) fn u16(&mut self) -> Result<u16, WireError> { Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    pub(crate) fn u32(&mut self) -> Result<u32, WireError> { Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    pub(crate) fn u64(&mut self) -> Result<u64, WireError> { Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap())) }
    pub(crate) fn i16(&mut self) -> Result<i16, WireError> { Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    pub(crate) fn i32(&mut self) -> Result<i32, WireError> { Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    pub(crate) fn id(&mut self) -> Result<DeviceId, WireError> { Ok(self.take(16)?.try_into().unwrap()) }
    pub(crate) fn string(&mut self, n: usize) -> Result<String, WireError> { String::from_utf8(self.take(n)?.to_vec()).map_err(|_| WireError::BadValue) }
    pub(crate) fn flag(&mut self) -> Result<bool, WireError> { match self.u8()? { 0 => Ok(false), 1 => Ok(true), _ => Err(WireError::BadValue) } }
}

impl Msg {
    /// 编成一帧（含开头的 4 字节长度）
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(vec![0, 0, 0, 0]);
        match self {
            Msg::Hello { protocol, id, os, name, width, height, port, proof } => {
                w.u8(1); w.u16(*protocol); w.bytes(id); w.u8(*os as u8); w.short_str(name); w.u32(*width); w.u32(*height); w.u16(*port); w.bytes(proof)
            }
            Msg::Size { width, height } => { w.u8(16); w.u32(*width); w.u32(*height) }
            Msg::FileBegin { xfer, files, clip } => { w.u8(17); w.u32(*xfer); w.u32(files.len() as u32); for (name, size) in files { w.short_str(name); w.u64(*size) } match clip { Some((t, k)) => { w.u8(if *k == ClipKind::Image { 1 } else { 2 }); w.u8(t.back as u8); w.u32(t.no) } None => w.u8(0) } }
            Msg::FileData { xfer, data } => { w.u8(18); w.u32(*xfer); w.u32(data.len() as u32); w.bytes(data) }
            Msg::FileEnd { xfer } => { w.u8(19); w.u32(*xfer) }
            Msg::FileCancel { xfer } => { w.u8(20); w.u32(*xfer) }
            Msg::FileAck { xfer, ok, err } => { w.u8(21); w.u32(*xfer); w.u8(*ok as u8); w.short_str(err) }
            Msg::Layout { rev, cells } => {
                w.u8(2); w.u64(*rev); w.u16(cells.len() as u16);
                for (id, x, y) in cells { w.bytes(id); w.i16(*x); w.i16(*y) }
            }
            Msg::Enter { side, permille, mods } => { w.u8(3); w.u8(*side as u8); w.u16(*permille); w.u32(*mods) }
            Msg::Leave => w.u8(4),
            Msg::MouseMove { dx, dy } => { w.u8(5); w.i32(*dx); w.i32(*dy) }
            Msg::MouseButton { button, down } => { w.u8(6); w.u8(*button); w.u8(*down as u8) }
            Msg::Wheel { dx, dy } => { w.u8(7); w.i32(*dx); w.i32(*dy) }
            Msg::Key { usage, down } => { w.u8(8); w.u16(*usage); w.u8(*down as u8) }
            Msg::ClipText { text, tag } => { w.u8(9); w.u8(tag.back as u8); w.u32(tag.no); w.long_str(text) }
            Msg::Ping { nonce } => { w.u8(10); w.u32(*nonce) }
            Msg::Pong { nonce } => { w.u8(11); w.u32(*nonce) }
            Msg::JoinAsk { msg } => { w.u8(23); w.short_str_bytes(msg) }
            Msg::JoinReply { msg } => { w.u8(24); w.short_str_bytes(msg) }
            Msg::JoinProve { mac } => { w.u8(25); w.bytes(mac) }
            Msg::JoinGrant { mac, code, key, rev } => { w.u8(26); w.bytes(mac); w.short_str(code); w.bytes(key); w.u64(*rev) }
            Msg::JoinDeny { why } => { w.u8(27); w.u8(*why) }
            Msg::Members { list } => { w.u8(29); w.u8(list.len().min(32) as u8); for (id, ip, port) in list.iter().take(32) { w.bytes(id); w.bytes(ip); w.u16(*port) } }
            Msg::Regroup { code, key, rev, gone } => { w.u8(28); w.short_str(code); w.bytes(key); w.u64(*rev); w.u8(gone.is_some() as u8); if let Some(g) = gone { w.bytes(g) } }
        }
        let len = (w.0.len() - 4) as u32;
        w.0[..4].copy_from_slice(&len.to_be_bytes());
        w.0
    }

    /// 解一帧的内容（不含开头的 4 字节长度）
    pub fn decode(body: &[u8]) -> Result<Msg, WireError> {
        let mut r = Reader { buf: body, at: 0 };
        let msg = match r.u8()? {
            1 => {
                let protocol = r.u16()?; let id = r.id()?;
                let os = match r.u8()? { 1 => Os::Mac, 2 => Os::Windows, 3 => Os::Linux, _ => return Err(WireError::BadValue) };
                let n = r.u16()? as usize; let name = r.string(n)?;
                if protocol != PROTOCOL { return Ok(Msg::Hello { protocol, id, os, name, width: 0, height: 0, port: 0, proof: [0; 32] }) }  // 版本不同：只读到这里，好让对方知道
                Msg::Hello { protocol, id, os, name, width: r.u32()?, height: r.u32()?, port: r.u16()?, proof: r.take(32)?.try_into().unwrap() }
            }
            2 => {
                let rev = r.u64()?; let n = r.u16()? as usize;
                let mut cells = Vec::with_capacity(n.min(64));
                for _ in 0..n { cells.push((r.id()?, r.i16()?, r.i16()?)) }
                Msg::Layout { rev, cells }
            }
            3 => {
                let side = match r.u8()? { 0 => Side::Left, 1 => Side::Right, 2 => Side::Top, 3 => Side::Bottom, _ => return Err(WireError::BadValue) };
                let permille = r.u16()?; if permille > 1000 { return Err(WireError::BadValue) }
                Msg::Enter { side, permille, mods: r.u32()? }
            }
            4 => Msg::Leave,
            5 => Msg::MouseMove { dx: r.i32()?, dy: r.i32()? },
            6 => Msg::MouseButton { button: r.u8()?, down: r.flag()? },
            7 => Msg::Wheel { dx: r.i32()?, dy: r.i32()? },
            8 => Msg::Key { usage: r.u16()?, down: r.flag()? },
            9 => { let tag = ClipTag { back: r.flag()?, no: r.u32()? }; let n = r.u32()? as usize; Msg::ClipText { text: r.string(n)?, tag } }
            10 => Msg::Ping { nonce: r.u32()? },
            11 => Msg::Pong { nonce: r.u32()? },
            23 => { let n = r.u8()? as usize; Msg::JoinAsk { msg: r.take(n)?.to_vec() } }
            24 => { let n = r.u8()? as usize; Msg::JoinReply { msg: r.take(n)?.to_vec() } }
            25 => Msg::JoinProve { mac: r.take(32)?.try_into().unwrap() },
            26 => {
                let mac = r.take(32)?.try_into().unwrap(); let n = r.u16()? as usize;
                if n > 32 { return Err(WireError::BadValue) }
                Msg::JoinGrant { mac, code: r.string(n)?, key: r.take(32)?.try_into().unwrap(), rev: r.u64()? }
            }
            27 => Msg::JoinDeny { why: r.u8()? },
            29 => {
                let n = r.u8()? as usize;
                if n > 32 { return Err(WireError::BadValue) }
                let mut list = Vec::with_capacity(n);
                for _ in 0..n { list.push((r.id()?, r.take(4)?.try_into().unwrap(), r.u16()?)) }
                Msg::Members { list }
            }
            28 => {
                let n = r.u16()? as usize;
                if n > 32 { return Err(WireError::BadValue) }
                let (code, key, rev) = (r.string(n)?, r.take(32)?.try_into().unwrap(), r.u64()?);
                Msg::Regroup { code, key, rev, gone: if r.flag()? { Some(r.id()?) } else { None } }
            }
            16 => Msg::Size { width: r.u32()?, height: r.u32()? },
            17 => {
                let xfer = r.u32()?; let n = r.u32()? as usize;
                if n > 1_000_000 { return Err(WireError::BadValue) }
                let mut files = Vec::with_capacity(n.min(1024));
                for _ in 0..n { let len = r.u16()? as usize; files.push((r.string(len)?, r.u64()?)) }
                let kind = match r.u8()? { 0 => None, 1 => Some(ClipKind::Image), 2 => Some(ClipKind::Files), _ => return Err(WireError::BadValue) };
                let clip = match kind { Some(k) => Some((ClipTag { back: r.flag()?, no: r.u32()? }, k)), None => None };
                Msg::FileBegin { xfer, files, clip }
            }
            18 => { let xfer = r.u32()?; let n = r.u32()? as usize; Msg::FileData { xfer, data: r.take(n)?.to_vec() } }
            19 => Msg::FileEnd { xfer: r.u32()? },
            20 => Msg::FileCancel { xfer: r.u32()? },
            21 => { let xfer = r.u32()?; let ok = r.flag()?; let n = r.u16()? as usize; Msg::FileAck { xfer, ok, err: r.string(n)? } }
            t => return Err(WireError::BadType(t)),
        };
        if r.at != body.len() { return Err(WireError::BadValue) }       // 后面还拖着多余的字节
        Ok(msg)
    }
}

/// 把网络上陆续到达的字节切成一帧一帧。数据可能半帧半帧地来，也可能几帧粘在一起。
#[derive(Default)]
pub struct FrameReader { buf: Vec<u8> }

impl FrameReader {
    pub fn push(&mut self, data: &[u8]) { self.buf.extend_from_slice(data) }

    /// 取出下一帧的内容；还没收全返回 Ok(None)
    pub fn next(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        if self.buf.len() < 4 { return Ok(None) }
        let len = u32::from_be_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_FRAME { return Err(WireError::TooLong) }
        if self.buf.len() < 4 + len { return Ok(None) }
        let body = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Msg> {
        vec![
            Msg::Hello { protocol: PROTOCOL, id: [7; 16], os: Os::Mac, name: "书房的 Mac mini".into(), width: 1920, height: 1080, port: 47800, proof: [3; 32] },
            Msg::Size { width: 2560, height: 1440 },
            Msg::FileBegin { xfer: 3, files: vec![("照片/一.jpg".into(), 12345), ("空的.txt".into(), 0)], clip: None },
            Msg::FileBegin { xfer: 4, files: vec![("clip.png".into(), 99)], clip: Some((ClipTag { back: true, no: 7 }, ClipKind::Image)) },
            Msg::FileBegin { xfer: 5, files: vec![("文件夹/a.txt".into(), 3)], clip: Some((ClipTag { back: false, no: 8 }, ClipKind::Files)) },
            Msg::FileData { xfer: 3, data: vec![1, 2, 3, 255] },
            Msg::FileEnd { xfer: 3 },
            Msg::FileCancel { xfer: 3 },
            Msg::FileAck { xfer: 3, ok: false, err: "磁盘满了".into() },
            Msg::Layout { rev: 42, cells: vec![([1; 16], 0, 0), ([2; 16], -1, 0), ([3; 16], 0, 1)] },
            Msg::Enter { side: Side::Left, permille: 512, mods: 0x0003 },
            Msg::Leave,
            Msg::MouseMove { dx: -37, dy: 1200 },
            Msg::MouseButton { button: 1, down: true },
            Msg::Wheel { dx: 0, dy: -120 },
            Msg::Key { usage: 0x04, down: false },
            Msg::ClipText { text: "复制过来的一段话 🙂".into(), tag: ClipTag { back: false, no: 1 } },
            Msg::ClipText { text: String::new(), tag: ClipTag { back: true, no: u32::MAX } },
            Msg::Ping { nonce: 0xDEADBEEF },
            Msg::Pong { nonce: 1 },
            Msg::JoinAsk { msg: vec![1; 33] },
            Msg::JoinReply { msg: vec![2; 33] },
            Msg::JoinProve { mac: [4; 32] },
            Msg::JoinGrant { mac: [5; 32], code: "K7M4P2XQ".into(), key: [6; 32], rev: 3 },
            Msg::JoinDeny { why: 1 },
            Msg::Members { list: vec![([3; 16], [192, 168, 1, 20], 47800), ([4; 16], [10, 0, 0, 7], 1)] },
            Msg::Regroup { code: "K7M4P2XQ".into(), key: [6; 32], rev: 4, gone: None },
            Msg::Regroup { code: "K7M4P2XQ".into(), key: [6; 32], rev: 5, gone: Some([9; 16]) },
        ]
    }

    #[test]
    fn 每种消息编码后都能原样解回来() {
        for m in samples() {
            let frame = m.encode();
            assert_eq!(u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize, frame.len() - 4);
            assert_eq!(Msg::decode(&frame[4..]), Ok(m));
        }
    }

    #[test]
    fn 数据零零碎碎地到也能切对() {
        let all: Vec<u8> = samples().iter().flat_map(|m| m.encode()).collect();
        for chunk in [1usize, 3, 7, 64, all.len()] {
            let mut fr = FrameReader::default();
            let mut got = Vec::new();
            for piece in all.chunks(chunk) {
                fr.push(piece);
                while let Some(body) = fr.next().unwrap() { got.push(Msg::decode(&body).unwrap()) }
            }
            assert_eq!(got, samples(), "每次到 {chunk} 字节");
        }
    }

    #[test]
    fn 坏数据会被拒绝而不是崩溃() {
        assert_eq!(Msg::decode(&[]), Err(WireError::Short));
        assert_eq!(Msg::decode(&[99]), Err(WireError::BadType(99)));
        assert_eq!(Msg::decode(&[4, 0]), Err(WireError::BadValue));                    // Leave 后面多了一个字节
        assert_eq!(Msg::decode(&[6, 1, 2]), Err(WireError::BadValue));                 // 按下/抬起只能是 0 或 1
        assert_eq!(Msg::decode(&[3, 9, 0, 0, 0, 0, 0, 0]), Err(WireError::BadValue));  // 没有第 9 条边
        assert_eq!(Msg::decode(&[9, 0, 0, 0, 5, b'a']), Err(WireError::Short));        // 说有 5 个字节，只来了 1 个
        let mut fr = FrameReader::default();
        fr.push(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(fr.next(), Err(WireError::TooLong));
        // 截断的每一种长度都只能是“不够”或“值不对”，不能崩
        for m in samples() { let f = m.encode(); for cut in 0..f.len() - 4 { assert!(Msg::decode(&f[4..4 + cut]).is_err()) } }
    }
}
