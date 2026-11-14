//! 假电脑：只有一台电脑时用来自测键鼠共享。
//!
//! 在同一台 Mac 上运行：`cargo run --release --example fake_peer`
//! 它会假装是同一网络里的一台 Windows 电脑（屏幕 1920×1080，端口 47801），
//! 一苇的菜单里会出现“发现‘假电脑（自测）’”。点“加入”，输入它启动时打印的校验码，它这边自动允许；
//! 之后把鼠标推出屏幕右边缘，鼠标就“穿”到它上面：本机鼠标消失，这里打印收到的鼠标、按键；
//! 往左推回来（或连按三下 Esc）鼠标就回来了。
//! 身份和所在的组存在临时文件夹的 yiwei_fake_peer.txt 里，下次启动不用重新加入。

use std::io::Write;
use std::sync::mpsc::channel;
use std::sync::Mutex;
use yiwei_core::input::{Action, Event};
use yiwei_core::net::{Config, Engine, Notice};
use yiwei_core::secure::{Group, Identity};
use yiwei_core::wire::Os;

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 { return None }
    let mut o = [0u8; N];
    for i in 0..N { o[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()? }
    Some(o)
}

fn main() {
    let file = std::env::temp_dir().join("yiwei_fake_peer.txt");
    // 第一行：私钥 公钥；第二行：所在的组
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut lines = text.lines();
    let identity = lines.next().and_then(|l| {
        let (a, b) = l.split_once(' ')?;
        Some(Identity { private: unhex(a)?, public: unhex(b)? })
    }).unwrap_or_else(|| Identity::generate().expect("生成身份"));
    let group = lines.next().and_then(Group::from_text).unwrap_or_else(|| Group::generate(1).expect("生成校验码"));
    let save = |group: &str| { let _ = std::fs::write(&file, format!("{} {}\n{group}\n", hex(&identity.private), hex(&identity.public))); };
    save(&group.to_text());

    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let cfg = Config { id: [0xfe; 16], name: "假电脑（自测）".into(), os: Os::Windows, width: 1920, height: 1080,
                       identity: identity.clone(), group: group.clone(), port: 47801, discover: true };
    let engine = match Engine::start(cfg, move |n| { let _ = tx.lock().unwrap().send(n); }) {
        Ok(e) => e,
        Err(e) => { eprintln!("启动失败（端口 47801 被占用？）：{e}"); std::process::exit(1) }
    };
    engine.set_recv_dir(std::env::temp_dir().join("一苇假电脑收到的"));
    println!("假电脑已启动：端口 {}，校验码 {}。到一苇菜单里点“加入”，输入这个校验码。按 Ctrl+C 退出。", engine.port(), group.code);

    let mut moves = 0u32;
    for n in rx {
        match n {
            Notice::Found { name, .. } => println!("发现：{name}"),
            Notice::JoinRequest { id, name, .. } => { println!("“{name}”输对了校验码，想加入：这边自动允许"); engine.confirm_join(id, true) }
            Notice::JoinFailed { name, why } => println!("没能加入“{name}”（原因 {why}）"),
            Notice::Group { code, save: text, joined } => { save(&text); println!("{}，校验码 {code}", if joined { "加入了别人的组" } else { "这一组换了校验码" }) }
            Notice::Connected { name, width, height, .. } => println!("已连上“{name}”（屏幕 {width}×{height}）"),
            Notice::Disconnected { .. } => println!("断开了"),
            Notice::Stalled { name } => println!("“{name}”一直不确认，断开重连"),
            Notice::Mismatch { name, theirs } => println!("“{name}”的版本不一样（它是第 {theirs} 版协议），连不了"),
            Notice::Resized { width, height, .. } => println!("对方屏幕变成 {width}×{height}"),
            Notice::RecvBegin { name, count, total, .. } => println!("开始收：{name} 等 {count} 个，共 {total} 字节"),
            Notice::RecvDone { ok, err, first, .. } => println!("收{}：{} {err}", if ok { "完了" } else { "失败" }, first.display()),
            Notice::SendProgress { .. } | Notice::SendDone { .. } | Notice::RecvProgress { .. } => {}
            Notice::Layout { cells, .. } => println!("摆法：{}", cells.iter().map(|(i, x, y)| format!("{}…({x},{y})", &hex(i)[..4])).collect::<Vec<_>>().join("  ")),
            Notice::ClipText { text, .. } => println!("收到剪贴板：{}", text.chars().take(60).collect::<String>()),
            Notice::ClipImage { path, .. } => { println!("收到剪贴板里的图片：{}", path.display()); let _ = std::fs::remove_file(path); }
            Notice::ClipFiles { paths, .. } => println!("收到复制的文件：{} 个", paths.len()),
            Notice::ClipFailed { .. } => println!("带剪贴板的图片、文件没带成"),
            Notice::Input { actions, .. } => for a in actions {
                match a {
                    Action::Motion { dx, dy } => { moves += 1; if moves % 20 == 1 { print!("\r鼠标移动 ({dx:4}, {dy:4})   "); let _ = std::io::stdout().flush(); } }
                    Action::Wheel { dx, dy } => println!("\n滚轮 {dx} {dy}"),
                    Action::Event(Event::Enter { side, permille, .. }) => println!("\n鼠标进来了：从{}边的 {permille}‰ 处", ["左", "右", "上", "下"][side as usize]),
                    Action::Event(Event::Leave) => println!("\n鼠标走了"),
                    Action::Event(Event::TakeBack { .. }) => println!("\n被请回去了"),
                    Action::Event(Event::Edge { side, permille, .. }) => println!("\n鼠标走到对方{}边的 {permille}‰ 处了", ["左", "右", "上", "下"][side as usize]),
                    Action::Event(Event::Key { usage, down }) => println!("\n按键 0x{usage:02X} {}", if down { "按下" } else { "抬起" }),
                    Action::Event(Event::Button { button, down }) => println!("\n鼠标键 {button} {}", if down { "按下" } else { "抬起" }),
                }
            },
        }
    }
}
