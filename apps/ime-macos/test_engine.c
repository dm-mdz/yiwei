// 引擎层自测：不经过界面，直接验证配置、词库和 Shift 切换。
#include "yw_rime.h"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

static uint64_t S;
static int fails = 0;
static char got[1024];

static void feed(const char *keys) {
  for (const char *p = keys; *p; p++) {
    yw_key(S, (unsigned char)*p, 0);
    YwState st; yw_state(S, &st);
    if (st.commit) strncat(got, st.commit, sizeof(got) - strlen(got) - 1);
    yw_free(&st);
  }
}
static void expect(const char *name, const char *want) {
  int ok = strcmp(got, want) == 0;
  printf("%s %-28s 得到「%s」%s%s%s\n", ok ? "通过" : "失败", name, got, ok ? "" : " 期望「", ok ? "" : want, ok ? "" : "」");
  if (!ok) fails++;
  got[0] = 0; yw_clear(S);
}
int main(int argc, char **argv) {
  if (argc < 3) return 2;
  yw_start(argv[1], argv[2], "");
  yw_join();
  S = yw_session();
  if (!S) { printf("失败 无法创建会话\n"); return 1; }

  feed("nihao "); expect("整词 nihao", "你好");
  feed("zhongwen "); expect("整词 zhongwen", "中文");
  feed("jintiantianqihenhao "); expect("整句", "今天天气很好");
  feed("nh "); expect("简拼 nh", "你好");
  feed("nihoa "); expect("纠错：字母敲反 nihoa", "你好");
  feed("zhognguo "); expect("纠错：字母敲反 zhognguo", "中国");
  feed("zhogguo "); expect("纠错：少敲一个 zhogguo", "中国");
  feed("keyii "); expect("纠错：连敲两下 keyii", "可以");
  feed("huilai "); expect("纠错不干扰正常输入", "回来");
  feed("ceshi,"); expect("逗号顶字上屏", "测试，");
  feed("."); expect("句号", "。");
  feed("?"); expect("问号", "？");
  feed("<"); expect("书名号直接上屏", "《");
  feed("lue "); expect("略：lue", "略");
  feed("lve "); expect("略：lve 也行", "略");
  feed("nve "); expect("虐：nve", "虐");
  { // 装了全套大词库时：扩展词库和腾讯词库（自动注音）里的词打得出
    char ext[1200]; snprintf(ext, sizeof ext, "%s/yiwei_ext.dict.yaml", argv[1]);
    FILE *f = fopen(ext, "rb");
    if (f) { fclose(f);
      feed("wangzherongyao "); expect("扩展词库的词", "王者荣耀");
      feed("xiazaidouyin "); expect("腾讯词库的词（自动注音）", "下载抖音"); } }
  feed("ni2"); { /* 数字选第二个候选 */ int ok = strlen(got) > 0 && strcmp(got, "你") != 0;
    printf("%s %-28s 得到「%s」\n", ok ? "通过" : "失败", "数字键选第二个候选", got); if (!ok) fails++; got[0] = 0; yw_clear(S); }

  // 候选数量
  yw_key(S, 'n', 0); yw_key(S, 'i', 0);
  { YwState st; yw_state(S, &st);
    int ok = st.num_candidates == 7 && st.preedit && strcmp(st.preedit, "ni") == 0;
    printf("%s %-28s 候选 %d 个，输入区「%s」\n", ok ? "通过" : "失败", "每页 7 个候选", st.num_candidates, st.preedit ? st.preedit : "");
    if (!ok) fails++; yw_free(&st); yw_clear(S); }

  // 候选横排：右键、Tab 往后移一个，左键往回
  { for (const char *p = "nihao"; *p; p++) yw_key(S, (unsigned char)*p, 0);
    yw_key(S, 0xff53, 0); YwState a; yw_state(S, &a); int h1 = a.highlighted; yw_free(&a);
    yw_key(S, 0xff09, 0); yw_state(S, &a); int h2 = a.highlighted; yw_free(&a);
    yw_key(S, 0xff51, 0); yw_state(S, &a); int h3 = a.highlighted; yw_free(&a);
    int ok = h1 == 1 && h2 == 2 && h3 == 1;
    printf("%s %-28s 高亮 %d %d %d\n", ok ? "通过" : "失败", "左右键和 Tab 在候选间移动", h1, h2, h3); if (!ok) fails++; yw_clear(S); }

  // 单击 Shift：切到英文；再单击：切回中文
  yw_key(S, 0xffe1, 1); yw_key(S, 0xffe1, 1 << 30);
  { int ok = yw_ascii(S); printf("%s %-28s\n", ok ? "通过" : "失败", "单击 Shift 切到英文"); if (!ok) fails++; }
  { bool handled = yw_key(S, 'a', 0); int ok = !handled; printf("%s %-28s\n", ok ? "通过" : "失败", "英文状态按键直通"); if (!ok) fails++; }
  yw_key(S, 0xffe1, 1); yw_key(S, 0xffe1, 1 << 30);
  { int ok = !yw_ascii(S); printf("%s %-28s\n", ok ? "通过" : "失败", "再单击 Shift 切回中文"); if (!ok) fails++; }

  // Shift+字母不触发切换
  yw_key(S, 0xffe1, 1); yw_key(S, 'A', 1); yw_key(S, 0xffe1, 1 << 30);
  { int ok = !yw_ascii(S); printf("%s %-28s\n", ok ? "通过" : "失败", "Shift+字母不误切换"); if (!ok) fails++; yw_clear(S); }

  // 输入一半时单击 Shift：字母原样上屏并切到英文
  feed("abc"); yw_key(S, 0xffe1, 1); yw_key(S, 0xffe1, 1 << 30);
  { YwState st; yw_state(S, &st); if (st.commit) strcat(got, st.commit); yw_free(&st); }
  { int ok = strcmp(got, "abc") == 0 && yw_ascii(S);
    printf("%s %-28s 得到「%s」\n", ok ? "通过" : "失败", "输入中单击 Shift 字母上屏", got); if (!ok) fails++; got[0] = 0; }

  // 纠错层：敲错时给出原意；正常输入和没打完的输入不打扰
  yw_set_ascii(S, false);
  { struct { const char *in; int place; const char *text; } cases[] = {
      { "nohao", 1, "你好" }, { "zhidso", -1, "知道" } /* 引擎第二个候选已经是它就不重复（0），否则放第二位（1） */, { "shenne", 1, "什么" }, { "migtian", 1, "明天" },
      { "xiagfa", 1, "想法" }, { "meiyiu", 1, "没有" }, { "womrn", 0, "我们" } /* 引擎第二个候选已经是它，不重复 */, { "nihao", 0, "" }, { "wom", 0, "" }, { "xianz", 0, "" }, { "nh", 0, "" },
      { "youmy", 0, "" }, { "zenmy", 0, "" }, { "weishme", 0, "" } /* 全拼简拼混着打不算手误 */ };
    for (int i = 0; i < 14; i++) {
      char fx[64] = "", tx[256] = "";
      for (const char *p = cases[i].in; *p; p++) yw_key(S, (unsigned char)*p, 0);
      int r = yw_fix(S, fx, sizeof fx, tx, sizeof tx);
      int ok = cases[i].place == -1 ? (r == 0 || (r == 1 && strcmp(tx, cases[i].text) == 0)) : (r == cases[i].place && strcmp(tx, cases[i].text) == 0);
      printf("%s 纠错 %-20s 位置 %d 「%s」\n", ok ? "通过" : "失败", cases[i].in, r, tx); if (!ok) fails++;
      yw_clear(S);
    } }
  { char fx[64] = "", tx[256] = "";
    for (const char *p = "nohao"; *p; p++) yw_key(S, (unsigned char)*p, 0);
    yw_fix(S, fx, sizeof fx, tx, sizeof tx); yw_apply_fix(S, fx);
    YwState st; yw_state(S, &st);
    int ok = st.commit && strcmp(st.commit, "你好") == 0 && !st.preedit;
    printf("%s %-28s 得到「%s」\n", ok ? "通过" : "失败", "选中纠错候选后上屏", st.commit ? st.commit : ""); if (!ok) fails++;
    yw_free(&st); yw_clear(S); }

  // 整句：排在最前面的（小词库时是按词频另算的整句，大词库时就是引擎首选）应当是这句话；短的、和引擎一样的不另出
  { struct { const char *in; int has; const char *text; } cases[] = {
      { "haikeyiba", 1, "还可以吧" }, { "wozhidaole", 1, "我知道了" }, { "nikeyibangwoma", 1, "你可以帮我吗" },
      { "nihao", 0, "" }, { "jintiantianqihenhao", 0, "" } };
    for (int i = 0; i < 5; i++) {
      char tx[256] = "";
      for (const char *p = cases[i].in; *p; p++) yw_key(S, (unsigned char)*p, 0);
      int r = yw_sentence(S, tx, sizeof tx);
      YwState st; yw_state(S, &st);
      const char *first = r ? tx : (st.num_candidates ? yw_cand(&st, 0) : "");
      int ok = cases[i].has ? strcmp(first, cases[i].text) == 0 : r == 0;
      printf("%s 整句 %-22s %s「%s」\n", ok ? "通过" : "失败", cases[i].in, r ? "另算 " : "引擎 ", first); if (!ok) fails++;
      yw_free(&st);
      yw_clear(S);
    } }

  yw_destroy(S); yw_finalize();
  printf(fails ? "\n有 %d 项失败\n" : "\n全部通过\n", fails);
  return fails ? 1 : 0;
}
