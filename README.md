# 一苇（Yiwei）

新开一台电脑时要装的那几样日常小工具，做成轻量的、按需安装的两个软件。MIT 许可。

| 软件 | 管什么 | 装在哪 |
|---|---|---|
| **一苇输入法**（macOS） | 拼音输入；单击 Shift 切中英文；敲错字母时在第二个候选给出纠正 | `/Library/Input Methods/Yiwei.app`（自己构建的放 `~/Library/Input Methods/`） |
| **一苇**（macOS） | 截图（⇧⌃A：框选、标注、复制或保存）；键鼠共享 | “应用程序”文件夹，在菜单栏右上角 |
| **一苇**（Windows） | 键鼠共享 | 一个 `Yiwei.exe`，放哪都行，图标在任务栏右下角 |

互不依赖，要哪个装哪个。

## 现在做到哪了

- macOS：输入法、截图（标注、点选窗口、长截屏、提取文字）、键鼠共享可用。
- Windows：键鼠共享可用。
- 键鼠共享在一台 Mac 加两台 Windows 上用过：几台电脑入一组互相连上，鼠标推过屏幕边缘就到另一台，复制的文字、图片、100MB 以内的文件跟着指针走，也可以直接发文件。
- 还没做：Linux 版（输入法、截图、键鼠共享都没开始）；边缘防误触（现在一推到边就穿过去）。

## 下载安装

在 [Releases](https://github.com/dm-mdz/yiwei/releases) 下载，要哪个装哪个：

- **一苇输入法**（`Yiwei-IME-版本号.dmg`）：打开后把 Yiwei.app 拖到旁边的“输入法”文件夹上，按提示输入电脑密码。然后在“系统设置 → 键盘 → 输入法”里点加号，添加“一苇”。
- **一苇**（`Yiwei-版本号.dmg`）：打开后把 Yiwei.app 拖到旁边的“应用程序”文件夹上，再打开它。第一次用截图要允许“屏幕录制”，用键鼠共享要允许“辅助功能”，它会带你去系统设置。
- **Windows 版**（`Yiwei-Windows-版本号.zip`）：解压后双击 `Yiwei.exe`。exe 还没有微软的签名，第一次打开会弹“Windows 已保护你的电脑”，点“更多信息 → 仍要运行”。

两个 Mac 安装包都经过苹果公证，可以直接打开。要求 macOS 14 以上、Apple 芯片的 Mac。

构建要用到：macOS 14 以上、Xcode 命令行工具（Swift）、Rust。想让构建脚本用你自己的签名身份或者工具链路径，在仓库根目录建一个 `local.env`（不进仓库），例如：

```sh
YIWEI_SIGN_ID="Developer ID Application: 你的名字 (XXXXXXXXXX)"   # 不写就用临时签名，每次重新构建后要重新授权屏幕录制和辅助功能
YIWEI_NOTARY_PROFILE=你的公证配置名                                # tools/release_mac.sh 打发布包时用；xcrun notarytool store-credentials 存的
LLVM_MINGW="$HOME/llvm-mingw"                                      # 编 Windows 版用
```

## 一苇输入法（macOS）

构建和安装：

```sh
cd apps/ime-macos
./build.sh                       # 生成 build/Yiwei.app
./test.sh                        # 引擎和纠错自测
cp -R build/Yiwei.app ~/Library/Input\ Methods/
```

第一次装好后，在“系统设置 → 键盘 → 输入法”里点加号，添加“一苇”，之后从菜单栏的输入法菜单切过去用。不想用了，在同一个地方把它移除即可。

- 单击 Shift：中英文切换，光标旁会闪一下“中/英”。
- 候选每页 7 个，数字键选；`=`、`.`、`]` 下一页，`-`、`,`、`[` 上一页。
- 左右键和 Tab 在候选之间移动，空格上屏高亮的那个。
- 敲错字母时，纠正后的结果会出现在候选里（第一个候选后面）。
- 用自带小词库时，三个字以上的短句会按词频另算一个整句放在最前面（例如 haikeyiba 得到“还可以吧”）；加装大词库后引擎自己的整句已经更准，不再另算。

### 词库（建议加装）

仓库自带的是 rime-pinyin-simp（Apache-2.0，6.5 万条，源自早年的安卓拼音输入法），装上就能打字，但词少、选词不够准。**建议加装雾凇拼音词库**（[rime-ice](https://github.com/iDvel/rime-ice)，长期维护的简体词库）：

```sh
tools/get_dict.sh            # 基础：常用字表 + 基础词库，下载约 17 MB，54 万词
tools/get_dict.sh full       # 全套：再加扩展词库和腾讯词库，下载约 46 MB，188 万词
cd apps/ime-macos && ./build.sh && ./test.sh
pkill -f "Input Methods/Yiwei.app"; rm -rf ~/Library/Input\ Methods/Yiwei.app
cp -R build/Yiwei.app ~/Library/Input\ Methods/
```

- 构建时 `tools/make_dict.py` 看 `third_party/rime-ice/` 里有哪些文件就用哪些；删掉这个文件夹重新构建，就换回自带的小词库。
- 换词库不影响个人词库（你打过、选过的词照样记着，越用越准）。
- 代价（全套，M 系列 Mac 实测）：安装包里词库 45 MB；换词库后第一次启动要编译约 4 秒（这几秒按键直通，短暂占用约 700 MB 内存）；之后常驻内存和小词库时一样，约 34 MB。56 句日常用语的整句首选，从对 45 句升到对 52 句。
- **许可**：雾凇拼音是 GPL-3.0，所以**不放进本仓库、也不随一苇的发布包分发**，由你自己下载。自己构建出来的 App 里会附上它的许可文件；如果你要把带着它的 App 再发给别人，须遵守 GPL-3.0（生成的词库文件头部已写明来源、版本和改动；这样的 App 不能上架 App Store）。
- 想用别的词库（例如 CC BY 4.0 的[万象拼音](https://github.com/amzxyz/rime-wanxiang)）：只要是 Rime 的 `dict.yaml` 格式、不带声调的全拼，照着 `tools/make_dict.py` 加一种来源即可；带声调的要先去掉声调。
- 敲错字母时的纠错，判断“是不是敲错了”始终用自带小词库的词频（它的字频、词频出自同一份语料），纠正后的字词由你装的词库给出。

## 一苇（菜单栏：截图、键鼠共享）

```sh
cd apps/yiwei-macos
./build.sh
cp -R build/Yiwei.app ~/Applications/ && open ~/Applications/Yiwei.app
```

点菜单栏上的芦苇图标：

- **截图**开关：打开后按 ⇧⌃A 截图；关掉后不占快捷键。第一次截图系统会要“屏幕录制”权限。
- **截图快捷键**：点一下，直接按新的组合键就改了（要带 ⌃、⌥ 或 ⌘，Esc 取消）。
- **截图保存到**：点一下选文件夹，默认是桌面。
- **键鼠共享**开关：一套键盘鼠标控制几台电脑，复制的文字、图片、文件（合起来 100MB 以内）跟着指针过去，直接粘贴（指针离开后，那台电脑的剪贴板恢复成原来的；更大的文件用下面的“发文件”）。“展开”里有个“鼠标被控切回”开关（默认关）：打开后，这台的指针在别的电脑上时，那台电脑自己的鼠标一动，这台的指针就切回这台。每台各设各的，只管这台去控制别人的时候。第一次打开要在“辅助功能”里打开一苇。下面是电脑的位置图（连着的两台之间有黄线）；点“展开”后可以拖卡片摆成桌上的样子、加入同一网络里发现的电脑、手动添加。几台电脑是一“组”，一组共用一个校验码（“展开”里能看到）：要加入别人，点“加入”、输入对方的校验码，对方点一次“允许”，之后和那一组的每台电脑都自动连上；右键别的电脑可以把它请出这一组。发现新电脑时“展开”旁有小红点。鼠标推出那条边就过去了，连按三下 Esc 马上回来；别的电脑的鼠标在这台上时，这台自己的键鼠照常能用（两边的鼠标动的是同一个指针）。
- 几台电脑上的一苇要是同一版，不一样时下拉里会提示该更新哪一台。出问题时看运行记录：Mac 在 `~/Library/Logs/Yiwei/share.log`，Windows 在 `Yiwei.exe` 旁边的 `一苇日志.txt`（接着上次往后写，最多 5MB，不记剪贴板内容）。
- **发文件**：在小图上右键别的电脑选“发送文件”（可多选文件、文件夹）；或左键点一下那台电脑（出现蓝框，下拉固定住、点别处不收起），再把文件从访达拖进下拉，或按 ⌘V 发送复制的文件；再点一下那台电脑取消选中；点菜单栏图标收起下拉。也可以把文件拖到菜单栏的一苇图标上，再拖到卡片上。进度在小图下面，可取消。对方直接收下（只有同一组的电脑能发），收完回一声“收好了”才算发完；弹通知，点一下在访达或资源管理器里打开。右键自己这台选“保存位置”可以改收文件的地方。
- **大写锁定键不延迟**开关：有些键盘的大写锁定键按得快会没反应，要按第二下，这是系统的防误触延迟；打开后去掉它。默认关。
- **开机时自动启动**开关。
- 退出一苇。

截图时：拖动框选，或者不拖、直接点一下选中鼠标下面的整个窗口；四边四角可以拉大小，框内可以拖动；工具条上有矩形、椭圆、箭头、画笔、马赛克、文字、提取文字、撤销重做、长截屏、保存、取消、完成；点“提取文字”后框里认出来的字可以直接拖选，⌘C 复制选中的（没选就复制全部），再点一次退出；回车或双击复制到剪贴板；Esc 或右键取消。

长截屏：框好区域后点工具条上带向下箭头的图标，然后在框里向下滚动页面，旁边的小窗会显示已经接了多长，滚完点“完成”或“保存”。只认向下滚；滚太快会提示没对上，往回滚一点再继续，或点“回到断点”让一苇替你滚回去。也可以点“自动滚动”让它匀速替你滚。这两个按钮要替你操作页面，第一次用时系统会要求在“辅助功能”里打开一苇。

框选整个窗口也可以：只有中间一栏在滚、两边侧栏不动时，会自动只接会滚动的那一栏。页面顶部、底部固定不动的栏会自动认出来，底栏只在长图最后出现一次。最长 20000 像素。

给了“辅助功能”权限后，点选还能选中通知横幅这类不是独立窗口的小块。

自检：`apps/yiwei-macos/build/Yiwei.app/Contents/MacOS/YiweiMain --selftest`（长截屏的拼接算法，不需要屏幕）。

## 配置文件

界面里不出现配置文件；平时在下拉里改就行，下拉会写入这些文件。想手改时先退出一苇，改完再打开。

macOS：`~/Library/Application Support/Yiwei/config.toml`

| 项 | 含义 | 默认 |
|---|---|---|
| `screenshot` | 截图开关（`true` / `false`） | `true` |
| `screenshot_hotkey` | 截图快捷键，如 `shift+ctrl+a`（要带 ctrl、alt 或 cmd，键用字母或数字） | `shift+ctrl+a` |
| `screenshot_save_dir` | 截图保存的文件夹 | `~/Desktop` |
| `key_mouse_share` | 键鼠共享开关 | `false` |
| `receive_dir` | 别的电脑发来的文件放在哪 | `~/Downloads/一苇` |
| `caps_lock_no_delay` | 大写锁定键不延迟 | `false` |

键鼠共享另有一个 `share.json`（同一文件夹，只有本人能读）：这台电脑的编号和身份密钥、所在的组（校验码和组钥匙）、组里的电脑、电脑的摆法。不要手改；删掉它等于退出这一组、从头来。

Windows：`%LOCALAPPDATA%\Yiwei\share.txt`，内容和 Mac 的 `share.json` 一样，另有 `enabled 1/0`（键鼠共享开关）、`recvdir 路径`（收到的文件放哪，默认“下载\一苇”）。开机自启写在当前用户的“启动”注册表项（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `Yiwei`）。

## 一苇（Windows：键鼠共享）

在 Mac 上交叉编译（需要 Rust 的 `x86_64-pc-windows-gnullvm` 标准库和 [llvm-mingw](https://github.com/mstorsjo/llvm-mingw)，路径见脚本）：

```sh
cd apps/yiwei-windows
./package.sh                     # 生成 build/yiwei-windows/一苇-Windows.zip（Yiwei.exe + 使用说明）
```

用法见 `apps/yiwei-windows/使用说明.txt`。Windows 上不摆位置：在 Mac 上拖好会自动同步过来。

单机自测（只有一台 Mac 时）：`cd core && cargo run --release --example fake_peer`，它假装是同一网络里的一台 Windows。

## 目录

| 目录 | 内容 |
|---|---|
| `apps/ime-macos` | 输入法外壳（Swift）、引擎衔接和纠错（C） |
| `apps/yiwei-macos` | 菜单栏、截图、键鼠共享的 macOS 外壳（Swift） |
| `apps/yiwei-windows` | 键鼠共享的 Windows 外壳（Rust，在 Mac 上交叉编译） |
| `core` | 键鼠共享的核心：协议、加密和入组、穿屏逻辑（Rust，三个系统共用） |
| `data/rime` | 拼音方案和标点配置 |
| `tools` | 图标生成、纠错规则生成、词库下载（`get_dict.sh`）和生成（`make_dict.py`） |
| `docs` | 需求分析设计书、界面稿 |
| `third_party` | librime（BSD-3-Clause）、rime-pinyin-simp 词库（Apache-2.0）；加装的雾凇拼音放在 `third_party/rime-ice/`（GPL-3.0，不入库） |
