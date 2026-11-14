// 整句效果对比：每行“拼音<Tab>期望的字”，比较引擎首选和词频整句各对了多少
#include "yw_rime.h"
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
  yw_start(argv[1], argv[2], ""); yw_join();
  uint64_t S = yw_session();
  char line[512]; int n = 0, eng_ok = 0, our_ok = 0, both = 0, fixed = 0, broken = 0;
  while (fgets(line, sizeof line, stdin)) {
    line[strcspn(line, "\n")] = 0; char *tab = strchr(line, '\t'); if (!tab) continue;
    *tab = 0; const char *want = tab + 1;
    for (const char *p = line; *p; p++) yw_key(S, (unsigned char)*p, 0);
    YwState st; yw_state(S, &st);
    const char *eng = st.num_candidates ? yw_cand(&st, 0) : "";
    char ours[256] = ""; int differs = yw_sentence(S, ours, sizeof ours);
    const char *final = differs ? ours : eng;
    int e = strcmp(eng, want) == 0, o = strcmp(final, want) == 0;
    n++; eng_ok += e; our_ok += o; both += e && o; fixed += !e && o; broken += e && !o;
    if (e != o || (!e && !o)) printf("%s  %-22s 期望「%s」 引擎「%s」 整句「%s」\n", o && !e ? "改对" : (e && !o ? "改错" : "都错"), line, want, eng, final);
    yw_free(&st); yw_clear(S);
  }
  printf("\n共 %d 句：引擎首选对 %d，换成词频整句后对 %d（改对 %d，改错 %d）\n", n, eng_ok, our_ok, fixed, broken);
  yw_destroy(S); yw_finalize(); return 0;
}
