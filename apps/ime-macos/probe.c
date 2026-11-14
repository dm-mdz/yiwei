// 打印若干输入的前两页候选，用来对比改规则前后的差别
#include "yw_rime.h"
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
  yw_start(argv[1], argv[2], ""); yw_join();
  uint64_t S = yw_session();
  char line[256];
  while (fgets(line, sizeof line, stdin)) {
    line[strcspn(line, "\n")] = 0; if (!*line) continue;
    for (const char *p = line; *p; p++) yw_key(S, (unsigned char)*p, 0);
    printf("%s\t", line);
    for (int page = 0; page < 2; page++) {
      YwState st; yw_state(S, &st);
      if (page == 0) printf("[%s]\t", st.preedit ? st.preedit : "");
      for (int i = 0; i < st.num_candidates; i++) printf("%s ", yw_cand(&st, i));
      int last = st.is_last_page; yw_free(&st);
      if (last || !yw_page(S, false)) break;
    }
    printf("\n"); yw_clear(S);
  }
  yw_destroy(S); yw_finalize(); return 0;
}
