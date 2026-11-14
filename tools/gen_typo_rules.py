#!/usr/bin/env python3
"""生成拼音纠错规则，写进 data/rime/yiwei.schema.yaml。

纠三种手误（都限定在一个音节之内）：
  1. 相邻两个字母敲反   hai -> ahi   hao -> hoa   zhong -> zhogn
  2. 少敲一个字母       zhong -> zhog   jiang -> jang
  3. 同一个字母连敲两下 hao -> hhao（只保留不会和正常输入混淆的）

没有语言模型来判断“用户更可能想打哪个”，所以这里从严：
一个错误拼法只要有可能出现在正常输入里，就不收。具体排除——
  · 本身能切成若干个正常音节（ani = a+ni，lie 本身就是音节）
  · 能切成“正常音节 + 一个声母简拼”（wom = wo+m，hhao = h+hao）
  · 是某个正常音节的结尾（前面的声母可以另算简拼，如 x+ia）
  · 能嵌在一串正常拼音中间：前面借上一个音节的尾巴，后面是下一个音节的开头
    （il 会出现在 hu|i·l|ai 里，把“回来”的候选里混进“护理”；tain 会出现在 tai·n|an 里）
改完用 apps/ime-macos/probe.c 对比常用词的候选，确认正常输入的结果一条都没变。

用法：python3 tools/gen_typo_rules.py
"""
import re, sys, os
from functools import lru_cache

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DICT = os.path.join(ROOT, "third_party/rime-pinyin-simp/pinyin_simp.dict.yaml")
SCHEMA = os.path.join(ROOT, "data/rime/yiwei.schema.yaml")
BEGIN, END = "    # >>> 纠错规则（由 tools/gen_typo_rules.py 生成，不要手改）", "    # <<< 纠错规则"
ERASED = {"hm", "m", "n", "ng"}
VOWELS = set("aeiouv")

def syllables():
    out, body = set(), False
    for line in open(DICT, encoding="utf-8"):
        line = line.rstrip("\n")
        if line == "...": body = True; continue
        if not body or not line or line.startswith("#"): continue
        cols = line.split("\t")
        if len(cols) < 2: continue
        out.update(cols[1].split())
    # 雾凇拼音比自带词库多出的几个音节也算正常音节，这样生成的规则两种词库都适用
    out |= {"biang", "cei", "rua"}
    return {s for s in out if re.fullmatch(r"[a-z]+", s)} - ERASED

SYL = syllables()
PREFIX = {s[:i] for s in SYL for i in range(1, len(s))}
SUFFIX = {s[i:] for s in SYL for i in range(1, len(s))}
MAXLEN = max(map(len, SYL))

@lru_cache(None)
def seg(s, cons_left, min_len):
    """s 能否切成正常音节（每个至少 min_len 个字母），其中最多夹 cons_left 个单独的声母。"""
    if not s: return True
    for n in range(1, min(MAXLEN, len(s)) + 1):
        if s[:n] in SYL and n >= min_len and seg(s[n:], cons_left, min_len): return True
    if cons_left and s[0] not in VOWELS and seg(s[1:], cons_left - 1, min_len): return True
    return False

# 可以从上一个音节“借”来的尾巴：去掉它之后，上一个音节剩下的部分仍是正常拼音（hui -> hu|i）
TAIL = {s[i:] for s in SYL for i in range(1, len(s)) if seg(s[:i], 0, 1)}

def embeddable(v):
    """v 能否嵌在一串正常拼音中间：v = 上个音节的尾巴 + 若干整音节 + 下个音节的开头。"""
    for i in range(len(v) + 1):
        if i and v[:i] not in TAIL: continue
        for j in range(i, len(v) + 1):
            if j < len(v) and v[j:] not in PREFIX: continue
            if seg(v[i:j], 0, 1): return True
    return False

def safe(v):
    if len(v) < 2 or v in SYL or v in SUFFIX: return False
    if embeddable(v): return False
    if len(v) >= 3 and seg(v, 1, 2): return False      # 正常音节 + 一个声母简拼
    return True

swap, drop, dbl = {}, {}, {}
for s in sorted(SYL):
    for k in range(len(s) - 1):
        if s[k] != s[k + 1] and safe(s[:k] + s[k + 1] + s[k] + s[k + 2:]): swap.setdefault(k, []).append(s)
    if len(s) >= 4:
        for k in range(1, len(s) - 1):                 # 不删第一个和最后一个：那是简拼和“打到一半”
            if safe(s[:k] + s[k + 1:]): drop.setdefault(k, []).append(s)
    for k in range(len(s)):
        if safe(s[:k] + s[k] + s[k:]): dbl.setdefault(k, []).append(s)

lines = [BEGIN]
for k, ss in sorted(swap.items()):
    lines.append("    - fuzz/^(?=(?:%s)$)([a-z]{%d})([a-z])([a-z])([a-z]*)$/$1$3$2$4/" % ("|".join(ss), k))
for k, ss in sorted(drop.items()):
    lines.append("    - fuzz/^(?=(?:%s)$)([a-z]{%d})[a-z]([a-z]*)$/$1$2/" % ("|".join(ss), k))
for k, ss in sorted(dbl.items()):
    lines.append("    - fuzz/^(?=(?:%s)$)([a-z]{%d})([a-z])([a-z]*)$/$1$2$2$3/" % ("|".join(ss), k))
lines.append(END)

text = open(SCHEMA, encoding="utf-8").read()
block = "\n".join(lines) + "\n"
if BEGIN in text:
    a, b = text.index(BEGIN), text.index(END) + len(END) + 1
    text = text[:a] + block + text[b:]
else:
    anchor = "    - derive/^([jqxy])u/$1v/\n"
    assert text.count(anchor) == 1
    text = text.replace(anchor, anchor + block)
open(SCHEMA, "w", encoding="utf-8").write(text)
n = lambda d: sum(map(len, d.values()))
print("音节 %d 个；敲反 %d、少敲 %d、连敲 %d 种错误拼法，共 %d 条规则" % (len(SYL), n(swap), n(drop), n(dbl), len(lines) - 2))
