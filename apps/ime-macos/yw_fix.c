// 一苇 · 纠错层（在引擎外面）
//
// 做法和成熟输入法一样分两步：
//   1. 把“用户原本可能想打的拼音”都列出来——和实际输入只差一处：
//      相邻两个字母敲反、多敲一个、少敲一个、按到键盘上相邻的键；
//   2. 用词库里的词频给每一种打分，和“照原样读”的得分比，明显更像人话才提示。
// “照原样读”包括三种正常情况，都不算手误：整串是完整拼音、末尾还没打完、全拼和简拼混着打。
// 纠错结果只作为第二个候选出现，不抢第一位。
#include "yw_rime.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define NEG (-1e30f)
#define MAXLEN 40
#define ABBR_COST 1.0f          // 一个音节只打声母（简拼）的代价

// ---------- 小工具：字符串哈希表（可扩容）、一次性内存块 ----------

typedef struct { const char *key; float val; } Slot;
typedef struct { Slot *slots; uint32_t mask, count; } Map;

static uint32_t hash(const char *s, int len) {
  uint32_t h = 2166136261u;
  for (int i = 0; i < len; i++) { h ^= (unsigned char)s[i]; h *= 16777619u; }
  return h;
}
static void map_init(Map *m, uint32_t cap) { m->slots = calloc(cap, sizeof(Slot)); m->mask = cap - 1; m->count = 0; }
static Slot *map_find(const Map *m, const char *s, int len) {
  uint32_t i = hash(s, len) & m->mask;
  for (;;) {
    Slot *e = &m->slots[i];
    if (!e->key || (strncmp(e->key, s, len) == 0 && e->key[len] == 0)) return e;
    i = (i + 1) & m->mask;
  }
}
static void map_grow(Map *m) {
  Map bigger; map_init(&bigger, (m->mask + 1) * 2);
  for (uint32_t i = 0; i <= m->mask; i++) {
    if (!m->slots[i].key) continue;
    *map_find(&bigger, m->slots[i].key, (int)strlen(m->slots[i].key)) = m->slots[i];
  }
  bigger.count = m->count; free(m->slots); *m = bigger;
}
static char *chunk; static size_t chunk_left;
static const char *keep(const char *s, int len) {          // 把字符串存进常驻内存
  if (chunk_left < (size_t)len + 1) { chunk_left = 1 << 20; chunk = malloc(chunk_left); }
  char *p = chunk; memcpy(p, s, len); p[len] = 0; chunk += len + 1; chunk_left -= len + 1;
  return p;
}
static void map_put_max(Map *m, const char *key, int len, float v) {
  Slot *e = map_find(m, key, len);
  if (e->key) { if (v > e->val) e->val = v; return; }
  e->key = keep(key, len); e->val = v;
  if (++m->count * 10 > (m->mask + 1) * 6) map_grow(m);
}

// ---------- 词库 ----------

static Map words;      // “ni hao” -> 词频对数
static Map abbrs;      // 简拼写法 “n hao”“ni h”“n h” -> 词频对数（已扣简拼代价）
static Map syls, prefixes;
static Map texts;      // “ni hao” -> 这串拼音最常见的那个词在 text_arr 里的下标（整句用）
static const char **text_arr; static int ntext;
static int loaded = 0; // 0 未加载，1 已加载，-1 加载失败

// 所有词的拼音连写（不带空格），按字母序排好，用来回答“以这几个字母开头的词里最常见的有多常见”
typedef struct { const char *joined; float val; } Joined;
static Joined *joined; static int njoined;
static float best1[26], best2[26 * 26];
static int cmp_joined(const void *a, const void *b) { return strcmp(((const Joined *)a)->joined, ((const Joined *)b)->joined); }

static float best_prefix(const char *t, int len) {
  if (len == 1) return best1[t[0] - 'a'];
  if (len == 2) return best2[(t[0] - 'a') * 26 + (t[1] - 'a')];
  int lo = 0, hi = njoined;
  while (lo < hi) { int mid = (lo + hi) / 2; if (strncmp(joined[mid].joined, t, len) < 0) lo = mid + 1; else hi = mid; }
  float best = NEG;
  for (int i = lo; i < njoined && strncmp(joined[i].joined, t, len) == 0; i++) if (joined[i].val > best) best = joined[i].val;
  return best;
}
static bool is_syl(const char *s, int len) { return map_find(&syls, s, len)->key != NULL; }
static bool is_prefix(const char *s, int len) { return map_find(&prefixes, s, len)->key != NULL; }
static bool is_zcs_h(const char *s) { return (s[0] == 'z' || s[0] == 'c' || s[0] == 's') && s[1] == 'h'; }

// 一个词的各种简拼写法：每个音节可以写全，也可以只写声母（zh/ch/sh 还可以只写第一个字母）
static void add_abbrs(char **syl, int *slen, int k, int idx, char *key, int klen, int nab, float lp) {
  if (idx == k) { if (nab) map_put_max(&abbrs, key, klen, lp - ABBR_COST * nab); return; }
  int base = klen; if (idx) key[base++] = ' ';
  memcpy(key + base, syl[idx], slen[idx]);
  add_abbrs(syl, slen, k, idx + 1, key, base + slen[idx], nab, lp);
  if (slen[idx] > 1) add_abbrs(syl, slen, k, idx + 1, key, base + 1, nab + 1, lp);
  if (slen[idx] > 2 && is_zcs_h(syl[idx])) add_abbrs(syl, slen, k, idx + 1, key, base + 2, nab + 1, lp);
}

// 读词库：每行“字词<Tab>拼音（空格分隔）<Tab>词频”。
bool ywfix_load(const char *dict_path) {
  if (loaded) return loaded > 0;
  loaded = -1;
  FILE *f = fopen(dict_path, "rb");
  if (!f) return false;
  fseek(f, 0, SEEK_END); long size = ftell(f); fseek(f, 0, SEEK_SET);
  char *buf = malloc(size + 2);
  if (!buf || fread(buf, 1, size, f) != (size_t)size) { fclose(f); free(buf); return false; }
  fclose(f); buf[size] = '\n'; buf[size + 1] = 0;
  char *p = strstr(buf, "\n...\n");
  if (!p) { free(buf); return false; }
  p += 5;

  typedef struct { char *code; int len; double w; char *text; int tlen; } Entry;
  int cap = 70000, n = 0; Entry *es = malloc(sizeof(Entry) * cap);
  double total = 0;
  while (*p) {
    char *eol = strchr(p, '\n'); if (!eol) break;
    char *t1 = memchr(p, '\t', eol - p);
    if (t1 && *p != '#') {
      char *code = t1 + 1, *t2 = memchr(code, '\t', eol - code), *end = t2 ? t2 : eol;
      double w = t2 ? atof(t2 + 1) : 0;
      bool ok = end > code && *code != ' ' && end[-1] != ' ';
      for (char *c = code; c < end && ok; c++) ok = (*c >= 'a' && *c <= 'z') || (*c == ' ' && c[1] != ' ');
      if (ok) {
        if (n == cap) { cap *= 2; es = realloc(es, sizeof(Entry) * cap); }
        *end = 0; es[n++] = (Entry){ code, (int)(end - code), w + 1, p, (int)(t1 - p) }; total += w + 1;
      }
    }
    p = eol + 1;
  }
  map_init(&words, 1 << 17); map_init(&abbrs, 1 << 18); map_init(&syls, 2048); map_init(&prefixes, 4096);
  map_init(&texts, 1 << 17); text_arr = malloc(sizeof(char *) * (n + 1)); ntext = 0;
  joined = malloc(sizeof(Joined) * (n + 1)); njoined = 0;
  for (int i = 0; i < 26; i++) best1[i] = NEG;
  for (int i = 0; i < 26 * 26; i++) best2[i] = NEG;
  for (int i = 0; i < n; i++) {
    char *syl[8]; int slen[8], k = 0; bool skip = false;
    for (char *s = es[i].code; *s;) {
      char *e = strchr(s, ' '); int len = e ? (int)(e - s) : (int)strlen(s);
      // 方案里删掉的几个“音节”（hm、m、n、ng）不算；太长的也不是拼音
      if (len > 6 || (len == 1 && (*s == 'm' || *s == 'n')) || (len == 2 && (!strncmp(s, "hm", 2) || !strncmp(s, "ng", 2)))) skip = true;
      if (k < 8) { syl[k] = s; slen[k] = len; }
      k++; s += len; if (*s == ' ') s++;
    }
    if (skip || k == 0) continue;
    float lp = (float)log(es[i].w / total);
    Slot *had = map_find(&words, es[i].code, es[i].len);
    if (!had->key || lp > had->val) {             // 同一串拼音里词频最高的那个词，记下它的字
      text_arr[ntext] = keep(es[i].text, es[i].tlen);
      Slot *t = map_find(&texts, es[i].code, es[i].len);
      if (t->key) t->val = (float)ntext; else map_put_max(&texts, es[i].code, es[i].len, (float)ntext);
      ntext++;
    }
    map_put_max(&words, es[i].code, es[i].len, lp);
    char j[64], key[64]; int jl = 0;
    for (char *c = es[i].code; *c && jl < 60; c++) if (*c != ' ') j[jl++] = *c;
    joined[njoined++] = (Joined){ keep(j, jl), lp };
    if (lp > best1[j[0] - 'a']) best1[j[0] - 'a'] = lp;
    if (jl >= 2 && lp > best2[(j[0] - 'a') * 26 + (j[1] - 'a')]) best2[(j[0] - 'a') * 26 + (j[1] - 'a')] = lp;
    if (k <= 4) add_abbrs(syl, slen, k, 0, key, 0, 0, lp);
    for (int a = 0; a < k && a < 8; a++) {
      if (is_syl(syl[a], slen[a])) continue;
      map_put_max(&syls, syl[a], slen[a], 0);
      for (int b = 1; b < slen[a]; b++) map_put_max(&prefixes, syl[a], b, 0);
    }
  }
  free(es); free(buf);
  if (syls.count == 0) return false;
  qsort(joined, njoined, sizeof(Joined), cmp_joined);
  loaded = 1;
  return true;
}

// ---------- 打分 ----------

static bool segmentable(const char *s, int n) {
  bool ok[MAXLEN + 2] = { true };
  for (int i = 0; i < n; i++) {
    if (!ok[i]) continue;
    for (int L = 1; L <= 6 && i + L <= n; L++) if (is_syl(s + i, L)) ok[i + L] = true;
  }
  return ok[n];
}

// 从 start 起往后拼一个词（最多 4 个音节），更新 dp。abbr 为真时，音节也可以只写声母。
static void expand(const char *s, int n, int start, int pos, int k, char *key, int klen, float *dp, int *cnt, bool abbr, bool any_abbr) {
  for (int L = 1; L <= 6 && pos + L <= n; L++) {
    bool full = is_syl(s + pos, L);
    bool ab = abbr && ((L == 1 && !strchr("aeiouv", s[pos])) || (L == 2 && is_zcs_h(s + pos)));
    if (!full && !ab) continue;
    int kl = klen;
    if (k) key[kl++] = ' ';
    memcpy(key + kl, s + pos, L); kl += L;
    bool now_abbr = any_abbr || !full;
    float v = NEG;
    if (!now_abbr) { Slot *w = map_find(&words, key, kl); if (w->key) v = w->val; }
    if (abbr) { Slot *a = map_find(&abbrs, key, kl); if (a->key && a->val > v) v = a->val; }
    if (v > NEG / 2 && dp[start] + v > dp[pos + L]) { dp[pos + L] = dp[start] + v; cnt[pos + L] = cnt[start] + 1; }
    if (k + 1 < 4) expand(s, n, start, pos + L, k + 1, key, kl, dp, cnt, abbr, now_abbr);
  }
}
// 把拼音串切成词，取词频乘积最大的切法（对数相加）。切不开返回 NEG。
static float score_dp(const char *s, int n, int *nwords, float *dp, bool abbr) {
  int cnt[MAXLEN + 2]; char key[48];
  for (int i = 0; i <= n; i++) { dp[i] = NEG; cnt[i] = 0; }
  dp[0] = 0;
  for (int i = 0; i < n; i++) if (dp[i] > NEG / 2) expand(s, n, i, i, 0, key, 0, dp, cnt, abbr, false);
  *nwords = cnt[n];
  return dp[n];
}

// “照原样读”的得分：整串读得通（允许简拼），或者前面读得通、末尾是某个词还没打完的开头。读不通返回 NEG。
static float score_as_typed(const char *s, int n) {
  float dp[MAXLEN + 2]; int w;
  float best = score_dp(s, n, &w, dp, true);
  for (int k = 0; k < n; k++) {
    if (dp[k] < NEG / 2 || n - k > 24) continue;
    float t = best_prefix(s + k, n - k);
    if (t > NEG / 2 && dp[k] + t > best) best = dp[k] + t;
  }
  return best;
}

// 是否只是“还没打完”：前面切得开，末尾是某个音节的开头。
static bool in_progress(const char *s, int n) {
  for (int k = n - 1; k >= 0 && n - k <= 5; k--)
    if (is_prefix(s + k, n - k) && (k == 0 || segmentable(s, k))) return true;
  return false;
}

// ---------- 整句 ----------
// 引擎自己组的整句有时不如“词频乘积最大”的切法（haikeyiba：引擎给“还可一把”，这里给“还可以吧”）。
// 这里不用另外的语言模型，只用词库里现成的词频。

typedef struct { float v; int prev; int nsyl; char key[40]; } Step;

static void walk(const char *s, int n, int start, int pos, int k, char *key, int klen, Step *st) {
  for (int L = 1; L <= 6 && pos + L <= n; L++) {
    if (!is_syl(s + pos, L)) continue;
    int kl = klen;
    if (k) key[kl++] = ' ';
    memcpy(key + kl, s + pos, L); kl += L;
    Slot *w = map_find(&words, key, kl);
    if (w->key && st[start].v + w->val > st[pos + L].v) {
      st[pos + L].v = st[start].v + w->val; st[pos + L].prev = start; st[pos + L].nsyl = st[start].nsyl + k + 1;
      memcpy(st[pos + L].key, key, kl); st[pos + L].key[kl] = 0;
    }
    if (k + 1 < 4) walk(s, n, start, pos + L, k + 1, key, kl, st);
  }
}

// 返回音节数（0 表示不适用：不是完整拼音、太短或太长）。out 里是整句的字。
int ywfix_sentence(const char *raw, char *out, int cap) {
  if (loaded <= 0 || !raw) return 0;
  int n = (int)strlen(raw);
  if (n < 2 || n > MAXLEN - 1) return 0;
  for (int i = 0; i < n; i++) if (raw[i] < 'a' || raw[i] > 'z') return 0;
  static Step st[MAXLEN + 2];
  for (int i = 0; i <= n; i++) { st[i].v = NEG; st[i].prev = -1; st[i].nsyl = 0; }
  st[0].v = 0;
  char key[48];
  for (int i = 0; i < n; i++) if (st[i].v > NEG / 2) walk(raw, n, i, i, 0, key, 0, st);
  if (st[n].v < NEG / 2) return 0;
  const char *parts[MAXLEN]; int np = 0;
  for (int at = n; at > 0; at = st[at].prev) {
    Slot *t = map_find(&texts, st[at].key, (int)strlen(st[at].key));
    if (!t->key) return 0;
    parts[np++] = text_arr[(int)t->val];
  }
  int used = 0; out[0] = 0;
  for (int i = np - 1; i >= 0; i--) {
    int len = (int)strlen(parts[i]);
    if (used + len + 1 > cap) return 0;
    memcpy(out + used, parts[i], len); used += len; out[used] = 0;
  }
  return st[n].nsyl;
}

// ---------- 列出可能的原意 ----------

static const char *NEIGHBORS[26] = {
  /*a*/ "qwsz", /*b*/ "vghn", /*c*/ "xdfv", /*d*/ "serfcx", /*e*/ "wsdr", /*f*/ "drtgvc", /*g*/ "ftyhbv",
  /*h*/ "gyujnb", /*i*/ "ujko", /*j*/ "huikmn", /*k*/ "jiolm", /*l*/ "kop", /*m*/ "njk", /*n*/ "bhjm",
  /*o*/ "iklp", /*p*/ "ol", /*q*/ "wa", /*r*/ "edft", /*s*/ "awedxz", /*t*/ "rfgy", /*u*/ "yhji",
  /*v*/ "cfgb", /*w*/ "qase", /*x*/ "zsdc", /*y*/ "tghu", /*z*/ "asx" };

typedef struct { float best; char text[MAXLEN + 4]; } Pick;

static void consider(Pick *pk, const char *v, int n, float penalty) {
  if (n < 2 || n > MAXLEN || !segmentable(v, n)) return;
  float dp[MAXLEN + 2]; int w;
  float sc = score_dp(v, n, &w, dp, false);
  if (sc > NEG / 2 && sc + penalty > pk->best) { pk->best = sc + penalty; memcpy(pk->text, v, n); pk->text[n] = 0; }
}

// 返回 0：不纠；1：有更可能的原意，写在 out 里（纠正后的拼音）。
int ywfix_suggest(const char *raw, char *out, int cap) {
  if (loaded <= 0 || !raw) return 0;
  int n = (int)strlen(raw);
  if (n < 2 || n > MAXLEN - 1 || cap < n + 2) return 0;
  bool vowel = false;
  for (int i = 0; i < n; i++) {
    if (raw[i] < 'a' || raw[i] > 'z') return 0;
    if (strchr("aeiouv", raw[i])) vowel = true;
  }
  if (!vowel) return 0;                               // 全是声母：纯简拼，不碰
  bool clean = segmentable(raw, n);
  if (clean && n < 4) return 0;                       // 说得通的短输入不打扰
  bool typing = !clean && in_progress(raw, n);
  float raw_score = score_as_typed(raw, n);           // 读不通（比如落单的 i、u、v）时是 NEG

  Pick pk = { NEG, "" };
  char v[MAXLEN + 4];
  for (int i = 0; i + 1 < n; i++) {                   // 敲反
    if (raw[i] == raw[i + 1]) continue;
    memcpy(v, raw, n); v[i] = raw[i + 1]; v[i + 1] = raw[i];
    consider(&pk, v, n, -3);
  }
  for (int i = 0; i < n; i++) {                       // 按到邻键
    memcpy(v, raw, n);
    for (const char *c = NEIGHBORS[raw[i] - 'a']; *c; c++) { v[i] = *c; consider(&pk, v, n, -5.5f); }
  }
  // 还没打完时，末尾多一个少一个字母属正常，不算手误，所以那时不动最后一个字母
  for (int i = 0; i < (typing ? n - 1 : n); i++) {    // 多敲一个（同一个字母连敲两下最常见）
    memcpy(v, raw, i); memcpy(v + i, raw + i + 1, n - i - 1);
    consider(&pk, v, n - 1, (i > 0 && raw[i] == raw[i - 1]) ? -3 : -6.5f);
  }
  for (int i = 0; i <= (typing ? n - 1 : n); i++) {   // 少敲一个
    memcpy(v, raw, i); memcpy(v + i + 1, raw + i, n - i);
    for (char c = 'a'; c <= 'z'; c++) { v[i] = c; consider(&pk, v, n + 1, -5); }
  }
  if (pk.best < NEG / 2 || pk.best <= raw_score + 1) return 0;   // 要比照原样读明显更常见才提示
  strcpy(out, pk.text);
  return 1;
}
