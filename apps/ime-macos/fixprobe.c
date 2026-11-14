// 纠错层的离线检查：每行一个输入，打印“放第几位、纠成什么拼音”
#include "yw_rime.h"
#include <stdio.h>
#include <string.h>
bool ywfix_load(const char *dict_path);
int ywfix_suggest(const char *raw, char *out, int cap);
int ywfix_sentence(const char *raw, char *out, int cap);
int main(int argc, char **argv) {
  if (!ywfix_load(argv[1])) { printf("词库读不出来\n"); return 1; }
  char line[256], out[64];
  while (fgets(line, sizeof line, stdin)) {
    line[strcspn(line, "\n")] = 0; if (!*line) continue;
    int r = ywfix_suggest(line, out, sizeof out);
    char sent[256]; int ns = ywfix_sentence(line, sent, sizeof sent);
    printf("%s\t%d\t%s\t%d\t%s\n", line, r, r ? out : "", ns, ns ? sent : "");
  }
  return 0;
}
