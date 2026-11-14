#!/usr/bin/env python3
"""生成输入法用的词库文件（构建时由 apps/ime-macos/build.sh 调用）。

用哪些词库，看 third_party/ 里有什么：
  · 只有自带的 rime-pinyin-simp（Apache-2.0，6.5 万条）：直接用它。开源仓库默认就是这样。
  · 另有 third_party/rime-ice/（雾凇拼音，GPL-3.0，用 tools/get_dict.sh 下载）：
      8105.dict.yaml 常用字表 + base.dict.yaml 基础词库        → 主词库（也给纠错、整句算词频用）
      ext.dict.yaml 扩展词库、tencent.dict.yaml 腾讯词库（可选） → 附加词库（只给引擎用）
    自带词库里那些生僻字（规范字表以外的）仍然保留在最后，原来打得出的字不会打不出。

输出：<目录>/yiwei.dict.yaml，以及有附加词库时的 <目录>/yiwei_ext.dict.yaml。
注音统一成本方案的写法：略 lue、虐 nue（雾凇写作 lve、nve）。

用法：python3 tools/make_dict.py <输出目录>
"""
import os, sys, hashlib, re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SIMP = os.path.join(ROOT, "third_party/rime-pinyin-simp/pinyin_simp.dict.yaml")
ICE = os.path.join(ROOT, "third_party/rime-ice")

def rows(path):
    """词库正文的每一行，拆成列。"""
    body = False
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line == "...": body = True; continue
            if not body or not line or line.startswith("#"): continue
            yield line.split("\t")

def norm(code):
    return " ".join({"lve": "lue", "nve": "nue"}.get(s, s) for s in code.split())

def stamp(paths):
    """来源文件的版本号和内容指纹。来源不变，生成的词库就一字不差，引擎不会无故重新编译。"""
    h, vers = hashlib.sha1(), []
    for p in paths:
        with open(p, "rb") as f:
            data = f.read()
        h.update(data)
        m = re.search(rb'^version:\s*"?([^"\n]+)', data[:200000], re.M)
        vers.append(m.group(1).decode().strip() if m else "?")
    return max(vers), h.hexdigest()[:10]

def header(name, version, note, extra=""):
    return f"# 由 tools/make_dict.py 生成，不要手改\n{note}---\nname: {name}\nversion: \"{version}\"\nsort: by_weight\n{extra}...\n\n"

ICE_NOTE = ("# 源自雾凇拼音 https://github.com/iDvel/rime-ice （版本 {ver}），许可 GPL-3.0，见同一安装包 licenses/ 里的许可全文。\n"
            "# 已修改：几个词库合并成本文件；注音 lve、nve 改写为 lue、nue；腾讯词库的词条注音留空由引擎自动注音；\n"
            "# 另并入 rime-pinyin-simp（Apache-2.0）里规范字表以外的单字和读音，权重记 0。\n")

def main(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    ice = {n: os.path.join(ICE, n + ".dict.yaml") for n in ("8105", "base", "ext", "tencent")}
    have = {n for n, p in ice.items() if os.path.exists(p)}
    main_path, ext_path = os.path.join(out_dir, "yiwei.dict.yaml"), os.path.join(out_dir, "yiwei_ext.dict.yaml")
    if os.path.exists(ext_path): os.remove(ext_path)

    if not {"8105", "base"} <= have:
        # 没有雾凇：用自带的小词库
        n = 0
        with open(main_path, "w", encoding="utf-8") as o:
            ver, fp = stamp([SIMP])
            o.write(header("yiwei", f"{ver}-{fp}", "# 源自 rime-pinyin-simp（Apache-2.0）\n"))
            for r in rows(SIMP):
                if len(r) >= 2: o.write("\t".join(r) + "\n"); n += 1
        print(f"词库：自带的 rime-pinyin-simp，{n} 条")
        return

    seen, texts, counts = set(), set(), {}
    def emit(o, src, coded=True):
        n = 0
        for r in rows(ice[src]):
            text = r[0]
            if coded:
                if len(r) < 2 or not r[1]: continue
                code, weight = norm(r[1]), (r[2] if len(r) > 2 else "0")
                if (text, code) in seen: continue
                seen.add((text, code)); texts.add(text)
                o.write(f"{text}\t{code}\t{weight}\n")
            else:
                if text in texts: continue                 # 前面已经有带注音的了
                texts.add(text)
                o.write(f"{text}\t\t{r[1] if len(r) > 1 else '0'}\n")   # 注音留空，引擎按字表自动注音
            n += 1
        counts[src] = n

    extras = [n for n in ("ext", "tencent") if n in have]
    ver, fp = stamp([ice[n] for n in ("8105", "base") + tuple(extras)] + [SIMP])
    note = ICE_NOTE.format(ver=ver)
    with open(main_path, "w", encoding="utf-8") as o:
        o.write(header("yiwei", f"{ver}-{fp}", note, "import_tables:\n  - yiwei_ext\n" if extras else ""))
        emit(o, "8105"); emit(o, "base")
        # 自带词库里雾凇没有的单字和读音（规范字表以外的字，以及“那 nei、说 yue”这类读音）排在最后：
        # 原来打得出的照样打得出。权重都记 0（不影响自动注音），按原来的词频从高到低写，常用的在前
        left = [(float(r[2]) if len(r) > 2 else 0.0, r[0], r[1]) for r in rows(SIMP)
                if len(r) >= 2 and len(r[0]) == 1 and r[1] not in ("n", "ng", "m", "hm") and (r[0], norm(r[1])) not in seen]
        left.sort(key=lambda e: -e[0])
        rare = 0
        for _, text, code in left:
            if (text, code) in seen: continue
            seen.add((text, code)); o.write(f"{text}\t{code}\t0\n"); rare += 1
    if extras:
        with open(ext_path, "w", encoding="utf-8") as o:
            o.write(header("yiwei_ext", f"{ver}-{fp}", note))
            if "ext" in have: emit(o, "ext")
            if "tencent" in have: emit(o, "tencent", coded=False)
    names = {"8105": "常用字", "base": "基础词库", "ext": "扩展词库", "tencent": "腾讯词库"}
    print("词库：雾凇拼音（" + "、".join(f"{names[k]} {v}" for k, v in counts.items()) + f"），另保留自带词库的生僻字和读音 {rare}，共 {sum(counts.values()) + rare} 条")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else ".")
