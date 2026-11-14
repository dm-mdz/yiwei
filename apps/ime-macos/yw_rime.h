// 一苇：对 librime 的一层很薄的 C 封装，给 Swift 外壳用。
#ifndef YW_RIME_H
#define YW_RIME_H
#include <stdbool.h>
#include <stdint.h>

#define YW_MAX_CAND 10

typedef struct {
  bool composing;
  bool ascii_mode;
  char *preedit;          // 正在输入的内容（UTF-8），没有则为 NULL
  int cursor_pos;         // 光标在 preedit 里的字节位置
  int num_candidates;
  int highlighted;
  char *candidates[YW_MAX_CAND];
  bool is_last_page;
  int page_no;
  char *commit;           // 本次要上屏的文字（UTF-8），没有则为 NULL
} YwState;

bool yw_start(const char *shared_dir, const char *user_dir, const char *log_dir);
bool yw_maintaining(void);
void yw_join(void);
void yw_finalize(void);

uint64_t yw_session(void);
void yw_destroy(uint64_t s);
bool yw_key(uint64_t s, int keycode, int mask);
void yw_state(uint64_t s, YwState *out);
void yw_free(YwState *st);
void yw_commit_composition(uint64_t s);
void yw_clear(uint64_t s);
bool yw_ascii(uint64_t s);
void yw_set_ascii(uint64_t s, bool v);
bool yw_select(uint64_t s, int index_on_page);
bool yw_page(uint64_t s, bool backward);
const char *yw_cand(const YwState *st, int i);

// 纠错（实现见 yw_fix.c）。看当前输入有没有更可能的“原意”：
// 返回 0 没有；1 有（外壳把它放在候选第二位）。fixed 是纠正后的拼音，text 是它对应的字词。
int yw_fix(uint64_t s, char *fixed, int fixed_cap, char *text, int text_cap);
// 整句（实现见 yw_fix.c）：按词频切出来的整句和引擎首选不一样时返回 1，text 是这个整句。
int yw_sentence(uint64_t s, char *text, int text_cap);
// 用户选了纠错候选：把输入换成纠正后的拼音并上屏。
bool yw_apply_fix(uint64_t s, const char *fixed);
#endif
