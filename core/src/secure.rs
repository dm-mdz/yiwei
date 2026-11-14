//! 加密和配对。
//!
//! · 每台电脑有一对长期密钥（身份）。两台连上后先做一次 Noise 握手（XX 模式），
//!   之后所有内容都加密，并且双方都知道了对方的身份公钥。
//! · 几台电脑是一“组”。一组有两样东西：校验码（8 位，给人看、给人输入）和组钥匙（随机 32 字节，人不用管）。
//!   组员之间连上后互相出示“我有组钥匙”的证明就算认得，不用人点任何东西。
//! · 加入一组：在要加入的那台上输入对方的校验码。两边用 SPAKE2 核对“我们知道的是同一个码”——
//!   码本身不在网络上传，旁听的人拿不到，冒充的人每连一次只能猜一个；对方的用户再点一次“允许”，
//!   然后把校验码和组钥匙通过加密连接交给新成员。
//! · 握手后的密钥同时给两条通道用：TCP（剪贴板、文件等）和 UDP（鼠标键盘）。
//!   两条通道的包号各用一半号段，同一把密钥下绝不重复。

use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, StatelessTransportState};
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};

const PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// TCP 这条通道的包号都带上最高位，和 UDP 的包号（从 1 往上数）永不相撞
const STREAM_BIT: u64 = 1 << 63;
/// Noise 一条消息最多 65535 字节；留出 16 字节校验
const CHUNK: usize = 60000;

#[derive(Debug, PartialEq)]
pub enum SecureError { Handshake, Decrypt, Format, JoinOrder }

/// 一台电脑的身份。private 要存在本机、不能外传；public 是别人认它用的
#[derive(Clone)]
pub struct Identity { pub private: [u8; 32], pub public: [u8; 32] }

impl Identity {
    pub fn generate() -> Result<Identity, SecureError> {
        let kp = Builder::new(PATTERN.parse().map_err(|_| SecureError::Handshake)?).generate_keypair().map_err(|_| SecureError::Handshake)?;
        Ok(Identity { private: kp.private.try_into().map_err(|_| SecureError::Handshake)?, public: kp.public.try_into().map_err(|_| SecureError::Handshake)? })
    }
}

/// 握手：发起方先说话，一共三句
pub struct Handshake { hs: HandshakeState, initiator: bool }

impl Handshake {
    fn build(id: &Identity, initiator: bool) -> Result<Handshake, SecureError> {
        let b = Builder::new(PATTERN.parse().map_err(|_| SecureError::Handshake)?).local_private_key(&id.private);
        let hs = if initiator { b.build_initiator() } else { b.build_responder() }.map_err(|_| SecureError::Handshake)?;
        Ok(Handshake { hs, initiator })
    }
    /// 发起方：返回握手对象和要发的第一句
    pub fn start(id: &Identity) -> Result<(Handshake, Vec<u8>), SecureError> {
        let mut h = Self::build(id, true)?;
        let first = h.say()?;
        Ok((h, first))
    }
    pub fn respond(id: &Identity) -> Result<Handshake, SecureError> { Self::build(id, false) }

    fn say(&mut self) -> Result<Vec<u8>, SecureError> {
        let mut buf = [0u8; 256];
        let n = self.hs.write_message(&[], &mut buf).map_err(|_| SecureError::Handshake)?;
        Ok(buf[..n].to_vec())
    }
    /// 收到对方一句；需要回话就返回要发的内容
    pub fn hear(&mut self, msg: &[u8]) -> Result<Option<Vec<u8>>, SecureError> {
        let mut buf = [0u8; 256];
        self.hs.read_message(msg, &mut buf).map_err(|_| SecureError::Handshake)?;
        if self.hs.is_handshake_finished() { return Ok(None) }
        Ok(Some(self.say()?))
    }
    pub fn finished(&self) -> bool { self.hs.is_handshake_finished() }

    pub fn finish(self) -> Result<Session, SecureError> {
        if !self.hs.is_handshake_finished() { return Err(SecureError::Handshake) }
        let peer: [u8; 32] = self.hs.get_remote_static().ok_or(SecureError::Handshake)?.try_into().map_err(|_| SecureError::Handshake)?;
        let hash = Sha256::digest(self.hs.get_handshake_hash()).into();
        let t = self.hs.into_stateless_transport_mode().map_err(|_| SecureError::Handshake)?;
        Ok(Session { t, peer, hash, initiator: self.initiator, out: 0, incoming: 0, buf: Vec::new() })
    }
}

/// 握手完成后的加密连接
pub struct Session {
    t: StatelessTransportState,
    peer: [u8; 32],
    hash: [u8; 32],
    initiator: bool,
    out: u64,
    incoming: u64,
    buf: Vec<u8>,
}

impl Session {
    /// 对方的身份公钥：拿它去查“这台电脑以前配对过没有”
    pub fn peer(&self) -> [u8; 32] { self.peer }
    pub fn is_initiator(&self) -> bool { self.initiator }
    /// 这次握手的指纹：两边一样，每次握手都不一样，并且绑着双方的身份
    pub fn hash(&self) -> [u8; 32] { self.hash }

    /// TCP 通道：把一段明文加密成可以直接发出去的字节（内部按 6 万字节一段切开）
    pub fn seal(&mut self, plain: &[u8]) -> Result<Vec<u8>, SecureError> {
        let mut out = Vec::with_capacity(plain.len() + plain.len() / CHUNK * 18 + 18);
        let mut cipher = vec![0u8; CHUNK + 16];
        for piece in plain.chunks(CHUNK) {
            self.out += 1;
            let n = self.t.write_message(STREAM_BIT | self.out, piece, &mut cipher).map_err(|_| SecureError::Decrypt)?;
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&cipher[..n]);
        }
        Ok(out)
    }
    /// TCP 通道：把收到的字节喂进来（可以零零碎碎地到），返回已经解开的明文。被改过或顺序不对的数据会报错。
    pub fn open(&mut self, data: &[u8]) -> Result<Vec<u8>, SecureError> {
        self.buf.extend_from_slice(data);
        let mut plain = Vec::new();
        let mut piece = vec![0u8; CHUNK + 16];
        loop {
            if self.buf.len() < 2 { break }
            let n = u16::from_be_bytes([self.buf[0], self.buf[1]]) as usize;
            if n < 16 || n > CHUNK + 16 { return Err(SecureError::Format) }
            if self.buf.len() < 2 + n { break }
            self.incoming += 1;
            let m = self.t.read_message(STREAM_BIT | self.incoming, &self.buf[2..2 + n], &mut piece).map_err(|_| SecureError::Decrypt)?;
            plain.extend_from_slice(&piece[..m]);
            self.buf.drain(..2 + n);
        }
        Ok(plain)
    }

    /// UDP 通道：一个包 = 8 字节包号 + 密文。包号由调用方给（输入通道自己的序号），每个包必须不同
    pub fn seal_datagram(&self, seq: u64, plain: &[u8]) -> Result<Vec<u8>, SecureError> {
        if seq == 0 || seq & STREAM_BIT != 0 || plain.len() > 1300 { return Err(SecureError::Format) }
        let mut out = vec![0u8; 8 + plain.len() + 16];
        out[..8].copy_from_slice(&seq.to_be_bytes());
        let n = self.t.write_message(seq, plain, &mut out[8..]).map_err(|_| SecureError::Decrypt)?;
        out.truncate(8 + n);
        Ok(out)
    }
    pub fn open_datagram(&self, packet: &[u8]) -> Result<(u64, Vec<u8>), SecureError> {
        if packet.len() < 8 + 16 || packet.len() > 8 + 1300 + 16 { return Err(SecureError::Format) }
        let seq = u64::from_be_bytes(packet[..8].try_into().unwrap());
        if seq == 0 || seq & STREAM_BIT != 0 { return Err(SecureError::Format) }
        let mut plain = vec![0u8; packet.len() - 8];
        let n = self.t.read_message(seq, &packet[8..], &mut plain).map_err(|_| SecureError::Decrypt)?;
        plain.truncate(n);
        Ok((seq, plain))
    }

}

fn digest(label: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new(); h.update(label);
    for p in parts { h.update((p.len() as u32).to_be_bytes()); h.update(p) }
    h.finalize().into()
}
/// 比较两段是不是一样，花的时间和“从第几个字节开始不一样”无关
pub fn same(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0 }

/// 一组电脑共用的：校验码、组钥匙、第几版（每换一次加一，组员只跟着更新的换）
#[derive(Clone, Debug, PartialEq)]
pub struct Group { pub code: String, pub key: [u8; 32], pub rev: u64 }

/// 校验码用的字符：去掉了容易看混的 0 O 1 I L
const CODE_CHARS: &[u8; 31] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

impl Group {
    /// 新的一组（一台电脑没加入别人时，自己就是一组）。rev 是这一版的版本号
    pub fn generate(rev: u64) -> Result<Group, SecureError> {
        let (mut key, mut r) = ([0u8; 32], [0u8; 16]);
        getrandom::getrandom(&mut key).map_err(|_| SecureError::Handshake)?;
        getrandom::getrandom(&mut r).map_err(|_| SecureError::Handshake)?;
        let code = (0..8).map(|i| CODE_CHARS[u16::from_be_bytes([r[2 * i], r[2 * i + 1]]) as usize % 31] as char).collect();
        Ok(Group { code, key, rev })
    }
    /// 把人输入的校验码规整一下：去掉空格和横线，小写变大写
    pub fn normalize(code: &str) -> String { code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect() }
    /// 存盘用的一行字（外壳原样存、原样传回来）
    pub fn to_text(&self) -> String { format!("{}:{}:{}", self.code, self.key.iter().map(|b| format!("{b:02x}")).collect::<String>(), self.rev) }
    pub fn from_text(t: &str) -> Option<Group> {
        let mut it = t.split(':');
        let (code, key, rev) = (it.next()?, it.next()?, it.next()?.parse().ok()?);
        if it.next().is_some() || key.len() != 64 || code.is_empty() || code.len() > 32 || Group::normalize(code) != code { return None }
        let mut k = [0u8; 32];
        for i in 0..32 { k[i] = u8::from_str_radix(key.get(2 * i..2 * i + 2)?, 16).ok()? }
        Some(Group { code: code.to_string(), key: k, rev })
    }
    /// 组员的证明：我有这一组的钥匙。绑着这次握手和“我是发起的还是应答的一方”——别人抄去用不上，也不能把我的原样弹回给我
    pub fn proof(&self, hash: &[u8; 32], initiator: bool) -> [u8; 32] { digest(b"yiwei member v1", &[&self.key, hash, &[initiator as u8]]) }
    /// 广播里带的记号：同组的认得出“这是自己人”，就自动去连；外人看不出是哪一组，更算不出钥匙
    pub fn tag(&self, id: &[u8; 16]) -> [u8; 8] { digest(b"yiwei tag v1", &[&self.key, id])[..8].try_into().unwrap() }
}

/// 加入一组时核对校验码。想加入的一方（joiner）：start → 把那一段发过去 → 收到对方的一段后 finish → 把 mac(true) 发过去；
/// 被找的一方：收到对方的一段后 start + finish，把自己的一段发回去 → 核对对方的 mac(true) → 用户允许后发 mac(false)。
/// 用的是 SPAKE2：两边的校验码不一样时，谁也得不到对方码的任何线索；每一步只能走一次
pub struct Join { state: Option<Spake2<Ed25519Group>>, hash: [u8; 32], key: Option<Vec<u8>> }

impl Join {
    pub fn start(code: &str, hash: &[u8; 32], joiner: bool) -> (Join, Vec<u8>) {
        let pw = Password::new(Group::normalize(code).as_bytes());
        let (a, b) = (SpakeId::new(b"yiwei joiner"), SpakeId::new(b"yiwei member"));
        let (state, msg) = if joiner { Spake2::<Ed25519Group>::start_a(&pw, &a, &b) } else { Spake2::<Ed25519Group>::start_b(&pw, &a, &b) };
        (Join { state: Some(state), hash: *hash, key: None }, msg)
    }
    pub fn finish(&mut self, theirs: &[u8]) -> Result<(), SecureError> {
        let state = self.state.take().ok_or(SecureError::JoinOrder)?;
        self.key = Some(state.finish(theirs).map_err(|_| SecureError::Format)?);
        Ok(())
    }
    /// 这次核对作废（校验码不对）：之后拿不到证明，也不能重来
    pub fn spend(&mut self) { self.state = None; self.key = None }
    /// 证明“我算出了和你一样的钥匙”（也就是校验码一样）。joiner：这是哪一方的证明。绑着这次握手，中间人转手用不上
    pub fn mac(&self, joiner: bool) -> Option<[u8; 32]> { Some(digest(b"yiwei join v1", &[self.key.as_ref()?, &self.hash, &[joiner as u8]])) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(a: &Identity, b: &Identity) -> (Session, Session) {
        let (mut ha, m1) = Handshake::start(a).unwrap();
        let mut hb = Handshake::respond(b).unwrap();
        let m2 = hb.hear(&m1).unwrap().unwrap();
        let m3 = ha.hear(&m2).unwrap().unwrap();
        assert_eq!(hb.hear(&m3).unwrap(), None);
        assert!(ha.finished() && hb.finished());
        (ha.finish().unwrap(), hb.finish().unwrap())
    }
    #[test]
    fn 握手后双方认得对方的身份() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (sa, sb) = connect(&a, &b);
        assert_eq!(sa.peer(), b.public);
        assert_eq!(sb.peer(), a.public);
    }

    #[test]
    fn tcp通道加密解密和防篡改() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (mut sa, mut sb) = connect(&a, &b);
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();      // 比一段大，会被切开
        let mut wire = sa.seal(b"hello").unwrap();
        wire.extend(sa.seal(&big).unwrap());
        assert!(!wire.windows(5).any(|w| w == b"hello"));                            // 线上看不到明文
        let mut got = Vec::new();
        for piece in wire.chunks(777) { got.extend(sb.open(piece).unwrap()) }        // 零零碎碎地到
        assert_eq!(&got[..5], b"hello");
        assert_eq!(&got[5..], &big[..]);
        // 反方向也通
        let back = sb.seal("收到".as_bytes()).unwrap();
        assert_eq!(sa.open(&back).unwrap(), "收到".as_bytes());
        // 改一个字节就解不开
        let mut bad = sa.seal(b"secret").unwrap();
        let last = bad.len() - 1; bad[last] ^= 1;
        assert_eq!(sb.open(&bad), Err(SecureError::Decrypt));
    }

    #[test]
    fn 重放和乱序的tcp数据会被拒绝() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (mut sa, mut sb) = connect(&a, &b);
        let first = sa.seal(b"one").unwrap();
        assert_eq!(sb.open(&first).unwrap(), b"one");
        assert_eq!(sb.open(&first), Err(SecureError::Decrypt));                      // 同一段再来一遍
    }

    #[test]
    fn udp通道加密解密和防篡改() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (sa, sb) = connect(&a, &b);
        let p1 = sa.seal_datagram(1, b"move 1").unwrap();
        let p2 = sa.seal_datagram(2, b"move 2").unwrap();
        assert_eq!(sb.open_datagram(&p2).unwrap(), (2, b"move 2".to_vec()));         // 乱序、丢包都不影响解密
        assert_eq!(sb.open_datagram(&p1).unwrap(), (1, b"move 1".to_vec()));
        let mut bad = p1.clone(); bad[10] ^= 1;
        assert_eq!(sb.open_datagram(&bad), Err(SecureError::Decrypt));
        let mut renumbered = p1.clone(); renumbered[7] = 9;                          // 改包号也不行
        assert_eq!(sb.open_datagram(&renumbered), Err(SecureError::Decrypt));
        assert_eq!(sa.open_datagram(&p1), Err(SecureError::Decrypt));                // 自己发的包自己解不开（两个方向密钥不同）
        assert_eq!(sa.seal_datagram(0, b"x"), Err(SecureError::Format));
        assert_eq!(sb.open_datagram(&[0u8; 5]), Err(SecureError::Format));
        // 和 TCP 通道同时用也互不干扰
        let (mut sa, mut sb) = (sa, sb);
        let s = sa.seal(b"clip").unwrap();
        assert_eq!(sb.open(&s).unwrap(), b"clip");
        assert_eq!(sb.open_datagram(&sa.seal_datagram(1, b"again").unwrap()).unwrap().1, b"again");
    }

    /// 走一遍加入：返回（被找的一方认不认对方的证明，想加入的一方认不认对方的证明）
    fn join(code_j: &str, code_t: &str, hj: &[u8; 32], ht: &[u8; 32]) -> (bool, bool) {
        let (mut j, m1) = Join::start(code_j, hj, true);
        let (mut t, m2) = Join::start(code_t, ht, false);
        t.finish(&m1).unwrap(); j.finish(&m2).unwrap();
        (same(&t.mac(true).unwrap(), &j.mac(true).unwrap()), same(&j.mac(false).unwrap(), &t.mac(false).unwrap()))
    }

    #[test]
    fn 校验码一样才加得进来() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (sa, sb) = connect(&a, &b);
        assert_eq!(sa.hash(), sb.hash());
        assert_eq!(join("k7m4-p2xq", "K7M4P2XQ", &sa.hash(), &sb.hash()), (true, true), "大小写、横线不算数");
        assert_eq!(join("K7M4P2XR", "K7M4P2XQ", &sa.hash(), &sb.hash()), (false, false));
        assert_eq!(join("", "K7M4P2XQ", &sa.hash(), &sb.hash()), (false, false));
        // 每一步只能走一次
        let (mut j, m) = Join::start("K7M4P2XQ", &sa.hash(), true);
        assert!(j.mac(true).is_none(), "还没核对完，拿不到证明");
        let (_, m2) = Join::start("K7M4P2XQ", &sb.hash(), false);
        j.finish(&m2).unwrap();
        assert_eq!(j.finish(&m2), Err(SecureError::JoinOrder));
        let _ = m;
    }

    #[test]
    fn 中间人夹在两头时_校验码对也加不进来() {
        // 中间人分别和两边握手，把核对校验码的话原样转手：两边握手的指纹不一样，证明对不上
        let (a, b, m) = (Identity::generate().unwrap(), Identity::generate().unwrap(), Identity::generate().unwrap());
        let (sa, _) = connect(&a, &m);
        let (_, sb) = connect(&m, &b);
        assert_ne!(sa.hash(), sb.hash());
        assert_eq!(join("K7M4P2XQ", "K7M4P2XQ", &sa.hash(), &sb.hash()), (false, false));
    }

    #[test]
    fn 组钥匙的证明只有同组的对得上() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (sa, sb) = connect(&a, &b);
        let (g, other) = (Group::generate(1).unwrap(), Group::generate(1).unwrap());
        assert_eq!(g.code.len(), 8);
        assert!(g.code.bytes().all(|c| CODE_CHARS.contains(&c)));
        assert_ne!(g.key, other.key);
        // 发起方出示的证明，应答方用同一把钥匙算得出来；别的组算不出来；也不能把对方的证明原样弹回去冒充
        assert_eq!(g.proof(&sa.hash(), true), g.proof(&sb.hash(), true));
        assert_ne!(g.proof(&sa.hash(), true), other.proof(&sb.hash(), true));
        assert_ne!(g.proof(&sa.hash(), true), g.proof(&sa.hash(), false));
        assert_eq!(g.tag(&[1; 16]), g.tag(&[1; 16]));
        assert_ne!(g.tag(&[1; 16]), g.tag(&[2; 16]));
        assert_ne!(g.tag(&[1; 16]), other.tag(&[1; 16]));
        // 存盘再读回来
        assert_eq!(Group::from_text(&g.to_text()), Some(g.clone()));
        assert_eq!(Group::from_text("abc"), None);
        assert_eq!(Group::from_text(&g.to_text().to_lowercase()), None);
        assert_eq!(Group::normalize(" k7m4-p2xq "), "K7M4P2XQ");
    }

    #[test]
    fn 握手的话被改过就失败() {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (mut ha, m1) = Handshake::start(&a).unwrap();
        let mut hb = Handshake::respond(&b).unwrap();
        let mut m2 = hb.hear(&m1).unwrap().unwrap();
        let n = m2.len() - 1; m2[n] ^= 1;
        assert!(ha.hear(&m2).is_err());
        assert!(Handshake::respond(&b).unwrap().hear(&[1, 2, 3]).is_err());
    }
}
