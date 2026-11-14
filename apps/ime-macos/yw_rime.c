#include "yw_rime.h"
#include <rime_api.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

static RimeApi *api = NULL;
static char dict_path[1024];
static bool big_dict = false;          // 用的是大词库（雾凇）：引擎自己的整句已经够准，不再另算整句
static RimeSessionId helper = 0;      // 只用来查“纠正后的拼音对应什么字”的会话

bool ywfix_load(const char *dict_path);
int ywfix_suggest(const char *raw, char *out, int cap);
int ywfix_sentence(const char *raw, char *out, int cap);

bool yw_start(const char *shared_dir, const char *user_dir, const char *log_dir) {
  api = rime_get_api();
  if (!api) return false;
  // 纠错和整句用的词频始终取自带的小词库：它的字、词频率出自同一份语料，判断“是不是敲错了”是按它调好的；
  // 大词库的权重是人工调的排序值，不适合拿来算概率。纠正后的字词仍由引擎（大词库）给出。
  snprintf(dict_path, sizeof dict_path, "%s/pinyin_simp.dict.yaml", shared_dir);
  char main_dict[1024]; struct stat st;
  snprintf(main_dict, sizeof main_dict, "%s/yiwei.dict.yaml", shared_dir);
  big_dict = stat(main_dict, &st) == 0 && st.st_size > 5 * 1024 * 1024;
  ywfix_load(dict_path);                // 启动时就读好，免得第一次按键时卡一下
  RIME_STRUCT(RimeTraits, traits);
  traits.shared_data_dir = shared_dir;
  traits.user_data_dir = user_dir;
  traits.distribution_name = "Yiwei";
  traits.distribution_code_name = "yiwei";
  traits.distribution_version = "0.1.0";
  traits.app_name = "rime.yiwei";
  traits.min_log_level = 2;
  traits.log_dir = log_dir;
  api->setup(&traits);
  api->initialize(&traits);
  return api->start_maintenance(False);
}
bool yw_maintaining(void) { return api && api->is_maintenance_mode(); }
void yw_join(void) { if (api) api->join_maintenance_thread(); }
void yw_finalize(void) { if (api) api->finalize(); }

uint64_t yw_session(void) {
  if (!api) return 0;
  RimeSessionId sid = api->create_session();
  // 候选是横着排的：左右键（和 Tab）在候选之间移动，上下键翻页
  if (sid) api->set_option(sid, "_horizontal", True);
  return (uint64_t)sid;
}
void yw_destroy(uint64_t s) { if (api && s) api->destroy_session((RimeSessionId)s); }
bool yw_key(uint64_t s, int keycode, int mask) {
  return api && s && api->process_key((RimeSessionId)s, keycode, mask);
}

void yw_state(uint64_t s, YwState *out) {
  memset(out, 0, sizeof(*out));
  if (!api || !s) return;
  RimeSessionId sid = (RimeSessionId)s;
  RIME_STRUCT(RimeCommit, commit);
  if (api->get_commit(sid, &commit)) {
    if (commit.text) out->commit = strdup(commit.text);
    api->free_commit(&commit);
  }
  RIME_STRUCT(RimeStatus, status);
  if (api->get_status(sid, &status)) {
    out->composing = status.is_composing;
    out->ascii_mode = status.is_ascii_mode;
    api->free_status(&status);
  }
  RIME_STRUCT(RimeContext, ctx);
  if (api->get_context(sid, &ctx)) {
    if (ctx.composition.length > 0 && ctx.composition.preedit) {
      out->preedit = strdup(ctx.composition.preedit);
      out->cursor_pos = ctx.composition.cursor_pos;
    }
    int n = ctx.menu.num_candidates;
    if (n > YW_MAX_CAND) n = YW_MAX_CAND;
    out->num_candidates = n;
    out->highlighted = ctx.menu.highlighted_candidate_index;
    out->is_last_page = ctx.menu.is_last_page;
    out->page_no = ctx.menu.page_no;
    for (int i = 0; i < n; i++)
      out->candidates[i] = strdup(ctx.menu.candidates[i].text ? ctx.menu.candidates[i].text : "");
    api->free_context(&ctx);
  }
}
void yw_free(YwState *st) {
  free(st->preedit); free(st->commit);
  for (int i = 0; i < YW_MAX_CAND; i++) free(st->candidates[i]);
  memset(st, 0, sizeof(*st));
}
void yw_commit_composition(uint64_t s) { if (api && s) api->commit_composition((RimeSessionId)s); }
void yw_clear(uint64_t s) { if (api && s) api->clear_composition((RimeSessionId)s); }
bool yw_ascii(uint64_t s) { return api && s && api->get_option((RimeSessionId)s, "ascii_mode"); }
void yw_set_ascii(uint64_t s, bool v) { if (api && s) api->set_option((RimeSessionId)s, "ascii_mode", v ? True : False); }
bool yw_select(uint64_t s, int i) { return api && s && api->select_candidate_on_current_page((RimeSessionId)s, (size_t)i); }
bool yw_page(uint64_t s, bool backward) { return api && s && api->change_page((RimeSessionId)s, backward ? True : False); }
const char *yw_cand(const YwState *st, int i) { return (i >= 0 && i < st->num_candidates) ? st->candidates[i] : NULL; }

int yw_fix(uint64_t s, char *fixed, int fixed_cap, char *text, int text_cap) {
  if (!api || !s || fixed_cap < 48 || text_cap < 8 || !ywfix_load(dict_path)) return 0;
  RimeSessionId sid = (RimeSessionId)s;
  if (api->get_option(sid, "ascii_mode")) return 0;
  const char *raw = api->get_input(sid);
  char typed[64];
  if (!raw || !*raw || strlen(raw) >= sizeof typed) return 0;
  strcpy(typed, raw);
  RIME_STRUCT(RimeContext, ctx);
  if (!api->get_context(sid, &ctx)) return 0;
  int place = 0;
  // 只在第一页、还没有选定前半句的时候纠错
  if (ctx.composition.sel_start == 0 && ctx.menu.page_no == 0) place = ywfix_suggest(typed, fixed, fixed_cap);
  if (place) {
    text[0] = 0;
    if (!helper || !api->find_session(helper)) helper = api->create_session();
    if (helper) {
      api->set_option(helper, "ascii_mode", False);
      if (api->set_input(helper, fixed)) {
        RIME_STRUCT(RimeContext, h);
        if (api->get_context(helper, &h)) {
          if (h.menu.num_candidates > 0 && h.menu.candidates[0].text) snprintf(text, text_cap, "%s", h.menu.candidates[0].text);
          api->free_context(&h);
        }
      }
      api->clear_composition(helper);
    }
    if (!text[0]) place = 0;
    for (int i = 0; place && i < ctx.menu.num_candidates; i++)       // 引擎自己已经给出同样的结果，就不重复
      if (ctx.menu.candidates[i].text && strcmp(ctx.menu.candidates[i].text, text) == 0) place = 0;
  }
  api->free_context(&ctx);
  return place;
}

bool yw_apply_fix(uint64_t s, const char *fixed) {
  if (!api || !s || !fixed || !*fixed) return false;
  RimeSessionId sid = (RimeSessionId)s;
  if (!api->set_input(sid, fixed)) return false;
  return api->select_candidate_on_current_page(sid, 0);
}

// 整句：三个字以上的完整拼音，如果按词频切出来的整句和引擎的首选不一样，写到 text 里并返回 1。
int yw_sentence(uint64_t s, char *text, int text_cap) {
  if (big_dict || !api || !s || text_cap < 16 || !ywfix_load(dict_path)) return 0;
  RimeSessionId sid = (RimeSessionId)s;
  if (api->get_option(sid, "ascii_mode")) return 0;
  const char *raw = api->get_input(sid);
  char typed[64];
  if (!raw || !*raw || strlen(raw) >= sizeof typed) return 0;
  strcpy(typed, raw);
  RIME_STRUCT(RimeContext, ctx);
  if (!api->get_context(sid, &ctx)) return 0;
  int ok = 0;
  if (ctx.composition.sel_start == 0 && ctx.menu.page_no == 0 && ctx.menu.num_candidates > 0) {
    int nsyl = ywfix_sentence(typed, text, text_cap);
    ok = nsyl >= 3;
    for (int i = 0; ok && i < ctx.menu.num_candidates && i < 1; i++)
      if (ctx.menu.candidates[i].text && strcmp(ctx.menu.candidates[i].text, text) == 0) ok = 0;
  }
  api->free_context(&ctx);
  return ok;
}
