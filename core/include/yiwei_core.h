// 一苇 · 键鼠共享核心给各系统外壳用的 C 接口（实现在 core/src/ffi.rs）。
// 坐标一律以屏幕左上角为原点，往右、往下为正。电脑编号是 32 个十六进制字符。
#ifndef YIWEI_CORE_H
#define YIWEI_CORE_H
#include <stdint.h>

// 发生了什么事：一行 JSON，t 字段是种类——
//   found        发现了不在本组的电脑        id name os ip port
//   group        本机所在的组变了：code 给用户看；save 请原样存起来，下次启动传给 ywk_start。
//                joined=1 是加入了别人的组（原来组里的电脑都断开了，请把它们和摆法从界面上拿掉）、
//                joined=0 是这一组换了校验码，或者第一次启动刚生成   code save joined
//   ask          有一台电脑输对了校验码，想加入这一组：请问用户允不允许，然后调 ywk_confirm   id name os
//   join_failed  本机想加入别人的组，没成。why：1 校验码不对 2 对方没同意 3 对方暂时不接受加入 4 连不上或等太久   name why
//   connected    可以用了                    id name os w h
//   disconnected 断开了                      id
//   mismatch     来连的那台电脑上的一苇和本机不是同一版，连不了：请提示用户两边换成同一版   name theirs mine（协议版本号）
//   stalled      发给这台电脑的按键、穿屏一直没得到确认（多半是它的防火墙挡着），已经断开它重连：请提示用户   name
//   guest        有一台电脑的鼠标要进来了（紧接着是输入回调的 kind 5）：id 是它的编号   id
//   clip         对方带来的剪贴板文字   id back no text
//                back=0：它的指针进到本机时带来的。只在它的指针在本机上的这段时间用：它走了，本机的剪贴板要恢复成它来之前的样子
//                        （可能比 kind 5 早到或晚到，按 id 记着）。text 为空表示它这次没有能带的，请把它上次带来的去掉
//                back=1：本机的指针在它上面时复制的，跟着指针回来了：放进本机的剪贴板
//                no：它第几次往外带。同一次带的文字和图片 no 相同（文字先到，图片到了并成一份）；比已经收到的更早的一次晚到了就不要
//   clip_image   对方带来的剪贴板图片，已经存成 path 这个文件（PNG 或 BMP），用完把文件删掉；back、no 同上   id back no path
//   clip_files   对方带来的复制的文件、文件夹，已经收在临时文件夹里：paths 是最上层的那几个的完整路径。
//                当成“复制了这几个文件”交给剪贴板（规则同上）。文件别删：核心在同一台电脑下次带文件来时清掉   id back no paths:[...]
//   clip_failed  带给 id 的图片、文件半路没带成（读不了、断开了）：别当成“带过了”，下次指针过去重新带   id
//   layout       电脑的摆法变了              rev cells:[[id,x,y],...]
//   grab         鼠标穿到别的电脑上了：藏起本机鼠标，键鼠输入改为报给核心   id
//   release      鼠标回来了：恢复本机键鼠，把鼠标放到 (x, y)
//   send_progress 发文件的进度               x id done total（字节）
//   send_done     发完了（或失败、取消了）     x id ok err
//   recv_begin    开始收对方发来的文件         x id name count total
//   recv_progress 收的进度                     x id done total
//   recv_done     收完了（或失败）             x id ok err first（收下的第一个文件或文件夹的完整路径） count total
// 回调在核心自己的线程上被调用（可能几个线程同时），也可能在调用 ywk_place、ywk_forget 的线程上当场被调用；
// 字符串只在回调期间有效。回调里不要调 ywk_stop。两个回调都可以传 NULL（不关心就不传）。
// ywk_stop 返回之后不会再调任何回调。
// grab、release 这两种通知和下面的输入回调，核心是按发生的先后一个一个调的（中间拿着它内部的一把锁）：
// 外壳看到的“穿走、回来、对方进来、对方走了”的顺序就是真实的顺序。代价是在这几种回调里不能调
// ywk_place、ywk_forget、ywk_confirm、ywk_new_code、ywk_join、ywk_set_size、ywk_restore_layout、ywk_clip、ywk_clip_image（会卡死），要调就换个线程。
typedef void (*ywk_notice_fn)(void *ctx, const char *json);

// 对方发来的输入，按顺序照做。kind：
//   1 把指针放到 (a, b)       2 滚轮 (a, b)：120 = 一行，正数往上、往右
//   3 按键 a=键位编号 b=1 按下/0 抬起（按住不放时控制方会一直发“按下”，被控方照做即可，自己不要再做自动重复）
//   4 鼠标键 a=哪个键（1 左 2 右 3 中）b=按下/抬起，先把鼠标移到 (c, d)
//   5 对方的鼠标进来了 a=从哪条边 b=千分比 c=按着的修饰键（1 Shift 2 Ctrl 4 Alt/Option 8 Cmd/Win），
//     再加 16 表示对方的大写锁定开着：请让本机的大小写跟它一致
//   6 对方的鼠标走了：松开它按着的所有键和鼠标键。a=0 正常走了（本机指针先藏起来放中间，本机鼠标一动再出现）、
//     a=1 请它回去了：换了一台来控制，或者本机的鼠标自己穿到别的电脑上了（指针照常显示）、a=2 它掉线了
// 核心保证：只有正在控制本机的那一台的输入才会到这里（5 和 6 成对出现，中间夹着 1–4），别的电脑发来的、晚到的都已经扔掉。
typedef void (*ywk_input_fn)(void *ctx, int32_t kind, int32_t a, int32_t b, int32_t c, int32_t d);

// 生成这台电脑的身份，返回“私钥 公钥”（各 64 个十六进制字符，空格隔开）。用完调 ywk_free。
char *ywk_new_identity(void);
void ywk_free(char *s);

// 启动。os：1 macOS、2 Windows、3 Linux。group：上次 group 通知里的 save，没有就传空串（核心会自己成一组，并发一次 group 通知）。
// 端口被占用等原因失败返回 NULL（刚 stop 完马上 start 不会撞上）。
// port 传 0 用默认端口；discover 非 0 表示在同一网络里广播自己。失败返回 NULL。
void *ywk_start(const char *id, const char *name, int32_t os, uint32_t width, uint32_t height,
                const char *private_key, const char *public_key, const char *group,
                uint16_t port, int32_t discover, ywk_notice_fn on_notice, ywk_input_fn on_input, void *ctx);
void ywk_stop(void *h);
uint16_t ywk_port(void *h);

// 几台电脑是一“组”，一组共用一个校验码。同组的电脑发现了就自动连上，不用调任何函数。
void ywk_join(void *h, const char *ip, uint16_t port, const char *code);  // 用户要加入这台电脑所在的组：code 是用户输入的、它的校验码
void ywk_confirm(void *h, const char *id, int32_t ok);             // 收到 ask 之后，用户点了允许（1）或不允许（0）
char *ywk_code(void *h);                                           // 本机这一组现在的校验码（8 位）。用完调 ywk_free
void ywk_new_code(void *h);                                        // 换一个校验码：连着的组员跟着换，没连着的要用新码重新加入
void ywk_forget(void *h, const char *id);                          // 把这台电脑请出这一组：断开、从摆法里拿掉，并且这一组换一个校验码
void ywk_place(void *h, const char *id, int32_t x, int32_t y);     // 把一台电脑摆到某个格子
void ywk_set_size(void *h, uint32_t width, uint32_t height);       // 本机屏幕大小变了（会告诉连着的电脑）
void ywk_restore_layout(void *h, uint64_t rev, const char *cells); // 启动后放回上次存下的摆法：“编号:x:y”用逗号隔开
// 剪贴板跟着指针走。back = 0：本机的指针进到 id 这台电脑，把本机剪贴板里的带给它（文字传空的 = 这次没有能带的）；
// back = 1：id 的指针在本机上时复制的，它的指针走了，带回给它。no：本机第几次往外带（每带一次加一；同一次带的文字和图片用同一个）
void ywk_clip(void *h, const char *id, const char *text, int32_t back, uint32_t no);
// 剪贴板里是图片时：外壳把它存成一个临时文件（PNG 或 BMP，最大 64MB），把路径交给核心；发完文件由核心删掉。
void ywk_clip_image(void *h, const char *id, const char *path, int32_t back, uint32_t no);
// 剪贴板里是复制的文件、文件夹时（paths：一行一个路径，原件不动）：合起来 100MB、2000 个以内才带。
// 返回 1 带了、0 太大或者多于 2000 个没带（请提示用户改用“发送文件”）、-1 没连着或者里面没有文件（不用提示）
int32_t ywk_clip_files(void *h, const char *id, const char *paths, int32_t back, uint32_t no);
uint32_t ywk_send_files(void *h, const char *id, const char *paths);   // 发文件、文件夹（一行一个路径），返回编号 x，0 表示没连上
void ywk_cancel_send(void *h, uint32_t x);                          // 不发了
void ywk_cancel_recv(void *h, const char *id, uint32_t x);         // 不收了
void ywk_set_recv_dir(void *h, const char *dir);                   // 收到的文件放哪（启动后先设一次）
void ywk_set_log(const char *path);                               // 打开运行记录（真机排查用）
void ywk_log(const char *text);                                   // 外壳记一行
// 排查“卡不卡”用：往轨迹里记一行（种类和六个数的意思见 core/src/log.rs）。只往内存里记，可以在监听回调里调
void ywk_trace(int32_t tag, double a, double b, double c, double d, double e, double f);

// —— 下面这几个是给键鼠监听回调用的 ——
// 本机直接接的键鼠的每一次输入都报给核心。自己注入的（对方发来的输入）一定不要报；别的程序模拟出来的由外壳定
// （Windows 外壳在鼠标在本机时不报；macOS 外壳照报）。这几个函数只读几个标志、
// 把事件放进队列就返回，不加密、不发网络、不等锁，可以放心在系统的监听回调里调；真正的处理在核心的后台线程里，
// 结果（穿走、回来）随后通过 grab / release 通知告诉外壳。
// 别的电脑的鼠标在本机上时，本机自己的键鼠照常能用（两边的鼠标动的是同一个指针，不分谁优先）：
// 本机鼠标的移动也要照常报（x、y 是指针现在的位置），核心据此让对方接着从指针现在的地方走。外壳不用自己判断。
// 返回 1 表示这次输入拿走了（发给正在控制的那台电脑），本机不要再处理；返回 0 表示留给本机。
// 在本机按下的键和鼠标键，抬起时即使鼠标已经穿走了也返回 0（抬起也留给本机，免得本机这个键一直按着）；
// 这样的键在鼠标穿走以后的自动重复返回 1，但哪边都不给。后台线程万一卡住、队列放满（1024 个），新的输入会丢掉并记进运行记录。
// 贴着屏幕边缘还往外推时，有的系统给的位置和移动量都不再变：外壳要自己把“往外推”的方向补进 dx、dy（比如 ±1），不然穿不过去。
void ywk_local_move(void *h, double x, double y, double dx, double dy);   // 鼠标在本机时：现在的位置和这次的移动量
void ywk_remote_move(void *h, double dx, double dy);                      // 鼠标在别的电脑上时（ywk_remote 返回 1）：移动量
int32_t ywk_key(void *h, int32_t usage, int32_t down);
int32_t ywk_button(void *h, int32_t button, int32_t down);
int32_t ywk_wheel(void *h, int32_t dx, int32_t dy);                     // 120 = 一行，正数往上、往右
int32_t ywk_remote(void *h);                                              // 鼠标现在是不是在别的电脑上
// 鼠标贴到屏幕边缘时（调 ywk_local_move 之前）、锁屏回来时调：mask 是系统说的现在真按着的修饰键，第 0–7 位依次是
// 左 Ctrl、左 Shift、左 Alt/Option、左 Win/Cmd、右 Ctrl、右 Shift、右 Alt/Option、右 Win/Cmd（键位 0xE0–0xE7）。
// 核心记着按着、系统说没按着的（抬起那一下没监听到），核心补一次抬起，穿屏时就不会带着它过去。
// 第 8 位（0x100）：这台电脑的大写锁定现在开着（穿屏时告诉对面）。
void ywk_sync_mods(void *h, uint32_t mask);
void ywk_set_take_back(void *h, int32_t on);                              // “鼠标被控切回”，默认关。开着时：本机的指针在别的电脑上，那台自己的鼠标一动，本机的指针就被请回来（只管本机去控制别人的时候）
void ywk_home(void *h);                                                   // 紧急拉回：鼠标马上回到本机（随后会收到 release）

// 键位换算：macOS 键码 <-> USB 键位编号；没有对应的返回 -1
int32_t ywk_usage_from_mac(int32_t keycode);
int32_t ywk_mac_from_usage(int32_t usage);
#endif
