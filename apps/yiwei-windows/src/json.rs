//! 读核心发来的一行 JSON（只认核心会发的那几种写法）
#[derive(Debug, Clone, PartialEq)]
pub enum J { Null, Bool(bool), Num(f64), Str(String), Arr(Vec<J>), Obj(Vec<(String, J)>) }

impl J {
    pub fn get(&self, k: &str) -> Option<&J> { if let J::Obj(v) = self { v.iter().find(|e| e.0 == k).map(|e| &e.1) } else { None } }
    pub fn str(&self, k: &str) -> String { match self.get(k) { Some(J::Str(s)) => s.clone(), _ => String::new() } }
    pub fn num(&self, k: &str) -> f64 { match self.get(k) { Some(J::Num(n)) => *n, _ => 0.0 } }
    pub fn arr(&self) -> &[J] { if let J::Arr(v) = self { v } else { &[] } }
    pub fn as_str(&self) -> &str { if let J::Str(s) = self { s } else { "" } }
    pub fn as_num(&self) -> f64 { if let J::Num(n) = self { *n } else { 0.0 } }
}

pub fn parse(s: &str) -> Option<J> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let v = value(&b, &mut i)?;
    Some(v)
}
fn ws(b: &[char], i: &mut usize) { while *i < b.len() && b[*i].is_whitespace() { *i += 1 } }
fn value(b: &[char], i: &mut usize) -> Option<J> {
    ws(b, i);
    match *b.get(*i)? {
        '{' => {
            *i += 1; let mut v = Vec::new();
            loop {
                ws(b, i);
                if b.get(*i) == Some(&'}') { *i += 1; return Some(J::Obj(v)) }
                let J::Str(k) = value(b, i)? else { return None };
                ws(b, i); if b.get(*i) != Some(&':') { return None } *i += 1;
                v.push((k, value(b, i)?));
                ws(b, i);
                match b.get(*i) { Some(',') => *i += 1, Some('}') => { *i += 1; return Some(J::Obj(v)) } _ => return None }
            }
        }
        '[' => {
            *i += 1; let mut v = Vec::new();
            loop {
                ws(b, i);
                if b.get(*i) == Some(&']') { *i += 1; return Some(J::Arr(v)) }
                v.push(value(b, i)?);
                ws(b, i);
                match b.get(*i) { Some(',') => *i += 1, Some(']') => { *i += 1; return Some(J::Arr(v)) } _ => return None }
            }
        }
        '"' => {
            *i += 1; let mut s = String::new();
            while let Some(&c) = b.get(*i) {
                *i += 1;
                match c {
                    '"' => return Some(J::Str(s)),
                    '\\' => {
                        let e = *b.get(*i)?; *i += 1;
                        match e {
                            'n' => s.push('\n'), 'r' => s.push('\r'), 't' => s.push('\t'), 'b' => s.push('\u{8}'), 'f' => s.push('\u{c}'),
                            'u' => {
                                let hex: String = b.get(*i..*i + 4)?.iter().collect(); *i += 4;
                                let mut code = u32::from_str_radix(&hex, 16).ok()?;
                                if (0xD800..0xDC00).contains(&code) && b.get(*i) == Some(&'\\') && b.get(*i + 1) == Some(&'u') {
                                    let lo: String = b.get(*i + 2..*i + 6)?.iter().collect();
                                    let lo = u32::from_str_radix(&lo, 16).ok()?; *i += 6;
                                    code = 0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                                }
                                s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            c => s.push(c),
                        }
                    }
                    c => s.push(c),
                }
            }
            None
        }
        't' => { *i += 4; Some(J::Bool(true)) }
        'f' => { *i += 5; Some(J::Bool(false)) }
        'n' => { *i += 4; Some(J::Null) }
        _ => {
            let st = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) { *i += 1 }
            b[st..*i].iter().collect::<String>().parse().ok().map(J::Num)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn 读得懂核心发的几种() {
        let j = parse(r#"{"t":"code","id":"ab","name":"书房的 \"Mac\"\n","os":1,"code":"012345"}"#).unwrap();
        assert_eq!(j.str("t"), "code");
        assert_eq!(j.str("name"), "书房的 \"Mac\"\n");
        assert_eq!(j.num("os"), 1.0);
        let l = parse(r#"{"t":"layout","rev":7,"cells":[["aa",0,0],["bb",-1,2]]}"#).unwrap();
        let cells = l.get("cells").unwrap().arr();
        assert_eq!(cells[1].arr()[1].as_num(), -1.0);
        assert_eq!(parse(r#"{"t":"clip","text":"你🙂"}"#).unwrap().str("text"), "你🙂");
    }
}
