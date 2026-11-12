use std::cell::RefCell;

thread_local! {
    static FILES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn add_file(name: &str) -> u32 {
    FILES.with(|f| {
        let mut f = f.borrow_mut();
        f.push(name.to_string());
        (f.len() - 1) as u32
    })
}

pub fn file_name(i: u32) -> String {
    FILES.with(|f| f.borrow().get(i as usize).cloned().unwrap_or_default())
}

pub fn error_at(file: u32, line: u32, msg: &str) -> ! {
    eprintln!("{}:{}: error: {}", file_name(file), line, msg);
    std::process::exit(1)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TK { Ident, Punct, Keyword, Str, Num, PPNum, Eof }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NumTy { Int, UInt, Long, ULong, Float, Double }

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TK,
    pub s: String,      // spelling
    pub file: u32,
    pub line: u32,
    pub bol: bool,      // first token on its line
    pub space: bool,    // preceded by whitespace
    pub ival: i64,      // Num: value; Str: element size (1 or 4)
    pub fval: f64,
    pub nty: NumTy,
    pub bytes: Vec<u8>, // Str: decoded contents without the trailing NUL
    pub hide: Vec<std::rc::Rc<str>>, // preprocessor hideset
}

impl Token {
    pub fn new(kind: TK, s: &str, file: u32, line: u32) -> Token {
        Token { kind, s: s.to_string(), file, line, bol: false, space: false, ival: 0, fval: 0.0, nty: NumTy::Int, bytes: vec![], hide: vec![] }
    }
    pub fn is(&self, s: &str) -> bool {
        (self.kind == TK::Punct || self.kind == TK::Keyword) && self.s == s
    }
}

const KEYWORDS: &[&str] = &[
    "return", "if", "else", "for", "while", "int", "sizeof", "char", "struct", "union", "short", "long", "void",
    "typedef", "_Bool", "enum", "static", "goto", "break", "continue", "switch", "case", "default", "extern",
    "_Alignof", "_Alignas", "do", "signed", "unsigned", "const", "volatile", "auto", "register", "restrict",
    "__restrict", "__restrict__", "_Noreturn", "float", "double", "typeof", "__typeof__", "__typeof", "inline",
    "__inline", "__inline__", "_Thread_local", "__thread", "_Atomic", "__attribute__", "__asm__", "__asm",
    "__signed__", "__signed", "__const", "__volatile__", "__volatile", "_Static_assert", "__alignof__",
];

const PUNCTS: &[&str] = &[
    "<<=", ">>=", "...", "==", "!=", "<=", ">=", "->", "+=", "-=", "*=", "/=", "++", "--", "%=", "&=", "|=", "^=",
    "&&", "||", "<<", ">>", "##",
];

fn is_ident1(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

fn is_ident2(c: u8) -> bool {
    is_ident1(c) || c.is_ascii_digit()
}

fn hexval(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'f' => (c - b'a' + 10) as u32,
        _ => (c - b'A' + 10) as u32,
    }
}

/// `p[*i]` is the character after a backslash.
fn read_escaped(p: &[u8], i: &mut usize) -> u32 {
    let c = p[*i];
    if (b'0'..=b'7').contains(&c) {
        let mut v = 0;
        let mut n = 0;
        while n < 3 && (b'0'..=b'7').contains(&p[*i]) {
            v = v * 8 + (p[*i] - b'0') as u32;
            *i += 1;
            n += 1;
        }
        return v;
    }
    *i += 1;
    if c == b'x' {
        let mut v: u32 = 0;
        while p[*i].is_ascii_hexdigit() {
            v = (v << 4) | hexval(p[*i]);
            *i += 1;
        }
        return v;
    }
    match c {
        b'a' => 7,
        b'b' => 8,
        b't' => 9,
        b'n' => 10,
        b'v' => 11,
        b'f' => 12,
        b'r' => 13,
        b'e' => 27,
        _ => c as u32,
    }
}

/// Decodes one UTF-8 character at `p[*i]`.
fn read_utf8(p: &[u8], i: &mut usize) -> u32 {
    let c = p[*i];
    let (n, mut v) = if c < 0x80 { (1, c as u32) } else if c >= 0xf0 { (4, (c & 7) as u32) } else if c >= 0xe0 { (3, (c & 15) as u32) } else { (2, (c & 31) as u32) };
    for k in 1..n {
        v = (v << 6) | (p[*i + k] & 0x3f) as u32;
    }
    *i += n;
    v
}

pub fn tokenize(src: &[u8], file: u32) -> Vec<Token> {
    // Splice backslash-newlines, keeping the line count intact.
    let mut p: Vec<u8> = Vec::with_capacity(src.len() + 8);
    let (mut i, mut pending) = (0, 0);
    while i < src.len() {
        let c = src[i];
        if c == b'\\' && src.get(i + 1) == Some(&b'\n') {
            i += 2;
            pending += 1;
        } else if c == b'\\' && src.get(i + 1) == Some(&b'\r') && src.get(i + 2) == Some(&b'\n') {
            i += 3;
            pending += 1;
        } else if c == b'\n' {
            p.push(b'\n');
            p.extend(std::iter::repeat_n(b'\n', pending));
            pending = 0;
            i += 1;
        } else {
            if c != b'\r' {
                p.push(c);
            }
            i += 1;
        }
    }
    p.extend_from_slice(b"\n\0\0\0\0");
    let n = p.len() - 4;

    let mut toks = Vec::new();
    let (mut i, mut line, mut bol, mut space) = (0usize, 1u32, true, false);
    while i < n {
        let c = p[i];
        if c == b'\n' {
            line += 1;
            bol = true;
            space = false;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == 0x0b || c == 0x0c {
            space = true;
            i += 1;
            continue;
        }
        if c == b'/' && p[i + 1] == b'/' {
            while p[i] != b'\n' {
                i += 1;
            }
            space = true;
            continue;
        }
        if c == b'/' && p[i + 1] == b'*' {
            let start = line;
            i += 2;
            while !(p[i] == b'*' && p[i + 1] == b'/') {
                if i >= n {
                    error_at(file, start, "unclosed block comment");
                }
                if p[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            space = true;
            continue;
        }

        let start = i;
        let mut t = Token::new(TK::Punct, "", file, line);
        t.bol = bol;
        t.space = space;
        bol = false;
        space = false;

        // String/char literal prefix
        let mut q = i;
        let mut wide = false;
        if c == b'u' && p[i + 1] == b'8' && p[i + 2] == b'"' {
            q = i + 2;
        } else if (c == b'L' || c == b'u' || c == b'U') && (p[i + 1] == b'"' || p[i + 1] == b'\'') {
            q = i + 1;
            wide = true;
        }

        if c.is_ascii_digit() || (c == b'.' && p[i + 1].is_ascii_digit()) {
            i += 1;
            loop {
                let d = p[i];
                if (d == b'+' || d == b'-') && matches!(p[i - 1], b'e' | b'E' | b'p' | b'P') {
                    i += 1;
                } else if d.is_ascii_alphanumeric() || d == b'.' || d == b'_' {
                    i += 1;
                } else {
                    break;
                }
            }
            t.kind = TK::PPNum;
        } else if p[q] == b'"' {
            i = q + 1;
            loop {
                let d = p[i];
                if d == b'"' {
                    break;
                }
                if d == b'\n' || i >= n {
                    error_at(file, line, "unclosed string literal");
                }
                let v = if d == b'\\' {
                    i += 1;
                    read_escaped(&p, &mut i)
                } else if wide {
                    read_utf8(&p, &mut i)
                } else {
                    i += 1;
                    d as u32
                };
                if wide {
                    t.bytes.extend_from_slice(&v.to_le_bytes());
                } else {
                    t.bytes.push(v as u8);
                }
            }
            i += 1;
            t.kind = TK::Str;
            t.ival = if wide { 4 } else { 1 };
        } else if p[q] == b'\'' {
            i = q + 1;
            let v = if p[i] == b'\\' {
                i += 1;
                read_escaped(&p, &mut i)
            } else {
                read_utf8(&p, &mut i)
            };
            while p[i] != b'\'' {
                if p[i] == b'\n' || i >= n {
                    error_at(file, line, "unclosed char literal");
                }
                i += 1;
            }
            i += 1;
            t.kind = TK::Num;
            t.ival = if wide { v as i64 } else { v as u8 as i8 as i64 };
        } else if is_ident1(c) {
            while is_ident2(p[i]) {
                i += 1;
            }
            t.kind = TK::Ident;
        } else if let Some(pp) = PUNCTS.iter().find(|pp| p[i..].starts_with(pp.as_bytes())) {
            i += pp.len();
        } else if c.is_ascii_punctuation() {
            i += 1;
        } else {
            error_at(file, line, "invalid token");
        }
        t.s = String::from_utf8_lossy(&p[start..i]).into_owned();
        toks.push(t);
    }
    let mut eof = Token::new(TK::Eof, "", file, line);
    eof.bol = true;
    toks.push(eof);
    toks
}

fn convert_number(t: &mut Token) {
    let s = t.s.to_ascii_lowercase();
    let b = s.as_bytes();
    let (radix, digits) = if s.starts_with("0x") {
        (16, &s[2..])
    } else if s.starts_with("0b") {
        (2, &s[2..])
    } else if b[0] == b'0' {
        (8, &s[..])
    } else {
        (10, &s[..])
    };
    let end = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
    let suffix = &digits[end..];
    let (u, l) = match suffix {
        "" => (false, false),
        "u" => (true, false),
        "l" | "ll" => (false, true),
        "ul" | "lu" | "ull" | "llu" => (true, true),
        _ => return convert_float(t, &s),
    };
    let mut v: u64 = 0;
    for c in digits[..end].chars() {
        v = v.wrapping_mul(radix as u64).wrapping_add(c.to_digit(radix).unwrap() as u64);
    }
    t.nty = if radix == 10 {
        if u && l { NumTy::ULong } else if l { NumTy::Long } else if u { if v >> 32 != 0 { NumTy::ULong } else { NumTy::UInt } } else if v >> 31 != 0 { NumTy::Long } else { NumTy::Int }
    } else if u && l {
        NumTy::ULong
    } else if l {
        if v >> 63 != 0 { NumTy::ULong } else { NumTy::Long }
    } else if u {
        if v >> 32 != 0 { NumTy::ULong } else { NumTy::UInt }
    } else if v >> 63 != 0 {
        NumTy::ULong
    } else if v >> 32 != 0 {
        NumTy::Long
    } else if v >> 31 != 0 {
        NumTy::UInt
    } else {
        NumTy::Int
    };
    t.kind = TK::Num;
    t.ival = v as i64;
}

fn convert_float(t: &mut Token, s: &str) {
    let (body, nty) = if let Some(b) = s.strip_suffix('f').filter(|_| !s.starts_with("0x")) {
        (b, NumTy::Float)
    } else if let Some(b) = s.strip_suffix('l') {
        (b, NumTy::Double)
    } else {
        (s, NumTy::Double)
    };
    let v = if let Some(hex) = body.strip_prefix("0x") {
        // Hexadecimal floating constant: 0x1.8p3
        let (mant, exp) = hex.split_once('p').unwrap_or((hex, "0"));
        let (ip, fp) = mant.split_once('.').unwrap_or((mant, ""));
        let mut v = 0f64;
        for c in ip.chars() {
            v = v * 16.0 + c.to_digit(16).unwrap_or(0) as f64;
        }
        let mut scale = 1.0 / 16.0;
        for c in fp.chars() {
            v += c.to_digit(16).unwrap_or(0) as f64 * scale;
            scale /= 16.0;
        }
        v * 2f64.powi(exp.parse::<i32>().unwrap_or(0))
    } else {
        match body.parse::<f64>() {
            Ok(v) => v,
            Err(_) => error_at(t.file, t.line, &format!("invalid numeric constant '{}'", t.s)),
        }
    };
    t.kind = TK::Num;
    t.nty = nty;
    t.fval = if nty == NumTy::Float { v as f32 as f64 } else { v };
}

/// Converts preprocessing tokens to parser tokens: keywords, numbers, and
/// concatenation of adjacent string literals.
pub fn convert(toks: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(toks.len());
    for mut t in toks {
        match t.kind {
            TK::Ident => {
                if t.s == "__extension__" {
                    continue;
                }
                if KEYWORDS.contains(&t.s.as_str()) {
                    t.kind = TK::Keyword;
                }
            }
            TK::PPNum => convert_number(&mut t),
            TK::Str => {
                if let Some(last) = out.last_mut().filter(|l| l.kind == TK::Str) {
                    let widen = |b: &[u8]| b.iter().flat_map(|&c| (c as u32).to_le_bytes()).collect::<Vec<u8>>();
                    if last.ival < t.ival {
                        last.bytes = widen(&last.bytes);
                        last.ival = t.ival;
                    } else if t.ival < last.ival {
                        t.bytes = widen(&t.bytes);
                    }
                    last.bytes.extend_from_slice(&t.bytes);
                    continue;
                }
            }
            _ => {}
        }
        out.push(t);
    }
    out
}
