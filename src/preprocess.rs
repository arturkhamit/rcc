// C preprocessor: macro expansion with hidesets (Dave Prosser's algorithm,
// as in chibicc), conditional inclusion and #include.
use crate::lex::{self, add_file, error_at, file_name, tokenize, NumTy, Token, TK};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const BUILTIN_DIR: &str = "<rcc>";
const BUILTIN_HEADERS: &[(&str, &str)] = &[
    ("stddef.h", include_str!("../include/stddef.h")),
    ("stdarg.h", include_str!("../include/stdarg.h")),
    ("stdbool.h", include_str!("../include/stdbool.h")),
    ("stdalign.h", include_str!("../include/stdalign.h")),
    ("stdnoreturn.h", include_str!("../include/stdnoreturn.h")),
    ("iso646.h", include_str!("../include/iso646.h")),
    ("float.h", include_str!("../include/float.h")),
];

const PREDEFINED: &str = "
#define __rcc__ 1
#define __STDC__ 1
#define __STDC_VERSION__ 201112L
#define __STDC_HOSTED__ 1
#define __STDC_NO_ATOMICS__ 1
#define __STDC_NO_COMPLEX__ 1
#define __STDC_NO_THREADS__ 1
#define __STDC_UTF_16__ 1
#define __STDC_UTF_32__ 1
#define _LP64 1
#define __LP64__ 1
#define __ELF__ 1
#define __x86_64 1
#define __x86_64__ 1
#define __amd64 1
#define __amd64__ 1
#define __linux 1
#define __linux__ 1
#define __gnu_linux__ 1
#define __unix 1
#define __unix__ 1
#define linux 1
#define unix 1
#define __CHAR_BIT__ 8
#define __SIZEOF_SHORT__ 2
#define __SIZEOF_INT__ 4
#define __SIZEOF_LONG__ 8
#define __SIZEOF_LONG_LONG__ 8
#define __SIZEOF_FLOAT__ 4
#define __SIZEOF_DOUBLE__ 8
#define __SIZEOF_LONG_DOUBLE__ 16
#define __SIZEOF_POINTER__ 8
#define __SIZEOF_SIZE_T__ 8
#define __SIZEOF_PTRDIFF_T__ 8
#define __SIZE_TYPE__ unsigned long
#define __PTRDIFF_TYPE__ long
#define __INTPTR_TYPE__ long
#define __UINTPTR_TYPE__ unsigned long
#define __WCHAR_TYPE__ int
#define __INT8_TYPE__ signed char
#define __INT16_TYPE__ short
#define __INT32_TYPE__ int
#define __INT64_TYPE__ long
#define __UINT8_TYPE__ unsigned char
#define __UINT16_TYPE__ unsigned short
#define __UINT32_TYPE__ unsigned int
#define __UINT64_TYPE__ unsigned long
#define __INT_LEAST8_TYPE__ signed char
#define __INT_LEAST16_TYPE__ short
#define __INT_LEAST32_TYPE__ int
#define __INT_LEAST64_TYPE__ long
#define __UINT_LEAST8_TYPE__ unsigned char
#define __UINT_LEAST16_TYPE__ unsigned short
#define __UINT_LEAST32_TYPE__ unsigned int
#define __UINT_LEAST64_TYPE__ unsigned long
#define __INTMAX_TYPE__ long
#define __UINTMAX_TYPE__ unsigned long
#define __CHAR16_TYPE__ unsigned short
#define __CHAR32_TYPE__ unsigned int
#define __FLT_MAX__ 3.40282347e+38F
#define __FLT_MIN__ 1.17549435e-38F
#define __FLT_EPSILON__ 1.19209290e-7F
#define __DBL_MAX__ 1.7976931348623157e+308
#define __DBL_MIN__ 2.2250738585072014e-308
#define __DBL_EPSILON__ 2.2204460492503131e-16
#define __WINT_TYPE__ unsigned int
#define __SCHAR_MAX__ 127
#define __SHRT_MAX__ 32767
#define __INT_MAX__ 2147483647
#define __LONG_MAX__ 9223372036854775807L
#define __LONG_LONG_MAX__ 9223372036854775807LL
#define __ORDER_LITTLE_ENDIAN__ 1234
#define __ORDER_BIG_ENDIAN__ 4321
#define __BYTE_ORDER__ 1234
#define __USER_LABEL_PREFIX__
";

struct Macro {
    objlike: bool,
    params: Vec<String>,
    va_name: Option<String>,
    body: Vec<Token>,
}

struct Cond {
    in_else: bool,
    included: bool,
}

pub struct Preprocessor {
    macros: HashMap<String, Rc<Macro>>,
    conds: Vec<Cond>,
    include_paths: Vec<String>,
    once: HashSet<String>,
    pushed: HashMap<String, Vec<Option<Rc<Macro>>>>,
    counter: i64,
    depth: usize,
    pub prelude: Vec<Token>, // tokens of include/builtins.h
    base_file: String,
}

fn err(t: &Token, msg: &str) -> ! {
    error_at(t.file, t.line, msg)
}

fn is_hash(t: &Token) -> bool {
    t.bol && t.kind == TK::Punct && t.s == "#"
}

/// Makes a single token from source text, positioned at `at`.
fn make_token(text: &str, at: &Token) -> Token {
    let mut t = tokenize(text.as_bytes(), at.file).swap_remove(0);
    t.line = at.line;
    t.bol = at.bol;
    t.space = at.space;
    t
}

/// Pops the rest of the current line.
fn read_line(inp: &mut Vec<Token>) -> Vec<Token> {
    let mut v = vec![];
    while !inp.last().unwrap().bol {
        v.push(inp.pop().unwrap());
    }
    v
}

impl Preprocessor {
    pub fn new(user_paths: Vec<String>, after_paths: Vec<String>) -> Preprocessor {
        let mut include_paths = user_paths;
        include_paths.push(BUILTIN_DIR.to_string());
        for d in ["/usr/local/include", "/usr/include/x86_64-linux-gnu", "/usr/include"] {
            include_paths.push(d.to_string());
        }
        include_paths.extend(after_paths);
        let mut pp = Preprocessor { macros: HashMap::new(), conds: vec![], include_paths, once: HashSet::new(), pushed: HashMap::new(), counter: 0, depth: 0, prelude: vec![], base_file: String::new() };
        let mut inp = tokenize(PREDEFINED.as_bytes(), add_file("<built-in>"));
        inp.reverse();
        pp.process(&mut inp, &mut vec![]);
        // Builtins written in C: their macros stay defined, their declarations
        // are handed to the parser ahead of the translation unit.
        let mut inp = tokenize(include_str!("../include/builtins.h").as_bytes(), add_file("<builtins>"));
        inp.reverse();
        let mut prelude = vec![];
        pp.process(&mut inp, &mut prelude);
        prelude.pop();
        pp.prelude = prelude;
        pp
    }

    pub fn define(&mut self, name: &str, body: &str) {
        let mut body = tokenize(body.as_bytes(), 0);
        body.pop();
        self.macros.insert(name.to_string(), Rc::new(Macro { objlike: true, params: vec![], va_name: None, body }));
    }

    pub fn undef(&mut self, name: &str) {
        self.macros.remove(name);
    }

    pub fn run(&mut self, path: &str) -> Vec<Token> {
        let src = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("rcc: {}: {}", path, e);
            std::process::exit(1)
        });
        self.base_file = path.to_string();
        let mut inp = tokenize(&src, add_file(path));
        inp.reverse();
        let mut out = vec![];
        self.process(&mut inp, &mut out);
        if !self.conds.is_empty() {
            err(out.last().unwrap(), "unterminated conditional directive");
        }
        out
    }

    fn is_defined(&self, name: &str) -> bool {
        self.macros.contains_key(name) || matches!(name, "__FILE__" | "__LINE__" | "__COUNTER__" | "__DATE__" | "__TIME__" | "__BASE_FILE__" | "__INCLUDE_LEVEL__" | "__TIMESTAMP__")
    }

    fn process(&mut self, inp: &mut Vec<Token>, out: &mut Vec<Token>) {
        loop {
            let t = inp.last().unwrap();
            if t.kind == TK::Eof {
                out.push(inp.pop().unwrap());
                return;
            }
            if !is_hash(t) {
                if !self.expand_macro(inp) {
                    out.push(inp.pop().unwrap());
                }
                continue;
            }
            inp.pop();
            if inp.last().unwrap().bol {
                continue; // null directive
            }
            let d = inp.pop().unwrap();
            match d.s.as_str() {
                "include" | "include_next" => self.include(inp, &d),
                "define" => self.read_define(inp),
                "undef" => {
                    let line = read_line(inp);
                    if let Some(n) = line.first() {
                        self.macros.remove(&n.s);
                    }
                }
                "if" => {
                    let v = self.eval_cond(inp, &d);
                    self.conds.push(Cond { in_else: false, included: v });
                    if !v {
                        self.skip_cond(inp);
                    }
                }
                "ifdef" | "ifndef" => {
                    let line = read_line(inp);
                    let Some(n) = line.first() else { err(&d, "macro name missing") };
                    let v = self.is_defined(&n.s) == (d.s == "ifdef");
                    self.conds.push(Cond { in_else: false, included: v });
                    if !v {
                        self.skip_cond(inp);
                    }
                }
                "elif" => {
                    let Some(c) = self.conds.last() else { err(&d, "stray #elif") };
                    if c.in_else {
                        err(&d, "#elif after #else");
                    }
                    if c.included {
                        read_line(inp);
                        self.skip_cond(inp);
                    } else if self.eval_cond(inp, &d) {
                        self.conds.last_mut().unwrap().included = true;
                    } else {
                        self.skip_cond(inp);
                    }
                }
                "else" => {
                    let Some(c) = self.conds.last_mut() else { err(&d, "stray #else") };
                    if c.in_else {
                        err(&d, "duplicate #else");
                    }
                    c.in_else = true;
                    let included = c.included;
                    c.included = true;
                    read_line(inp);
                    if included {
                        self.skip_cond(inp);
                    }
                }
                "endif" => {
                    if self.conds.pop().is_none() {
                        err(&d, "stray #endif");
                    }
                    read_line(inp);
                }
                "pragma" => {
                    let line = read_line(inp);
                    let arg = |i: usize| line.get(i).map(|t| t.s.as_str()).unwrap_or("");
                    let name = arg(2).trim_matches('"').to_string();
                    match arg(0) {
                        "once" => {
                            self.once.insert(file_name(d.file));
                        }
                        "push_macro" => self.pushed.entry(name.clone()).or_default().push(self.macros.get(&name).cloned()),
                        "pop_macro" => match self.pushed.get_mut(&name).and_then(|v| v.pop()) {
                            Some(Some(m)) => {
                                self.macros.insert(name, m);
                            }
                            Some(None) => {
                                self.macros.remove(&name);
                            }
                            None => {}
                        },
                        // Passed on to the parser as `__pragma_pack ( ... )`.
                        "pack" => {
                            out.push(make_token("__pragma_pack", &d));
                            out.extend(line.into_iter().skip(1));
                        }
                        _ => {}
                    }
                }
                "line" => {
                    // Renumber the rest of this file. The file name operand is ignored.
                    let line = self.expand_all(read_line(inp));
                    let Some(n) = line.first().and_then(|t| t.s.parse::<u32>().ok()) else { err(&d, "invalid #line directive") };
                    let delta = n.wrapping_sub(d.line + 1);
                    for t in inp.iter_mut().rev().take_while(|t| t.file == d.file) {
                        t.line = t.line.wrapping_add(delta);
                    }
                }
                "__include_end" => self.depth -= 1,
                "ident" | "warning" => {
                    read_line(inp);
                }
                "error" => {
                    let msg: Vec<String> = read_line(inp).into_iter().map(|t| t.s).collect();
                    err(&d, &format!("#error {}", msg.join(" ")));
                }
                _ => err(&d, &format!("invalid preprocessing directive #{}", d.s)),
            }
        }
    }

    /// Skips to the #elif, #else or #endif matching the current conditional.
    fn skip_cond(&mut self, inp: &mut Vec<Token>) {
        let mut depth = 0;
        loop {
            let n = inp.len();
            let t = &inp[n - 1];
            if t.kind == TK::Eof {
                err(t, "unterminated conditional directive");
            }
            if is_hash(t) && !inp[n - 2].bol {
                match inp[n - 2].s.as_str() {
                    "if" | "ifdef" | "ifndef" => depth += 1,
                    "elif" | "else" if depth == 0 => return,
                    "endif" if depth == 0 => return,
                    "endif" => depth -= 1,
                    _ => {}
                }
                inp.pop();
            }
            inp.pop();
        }
    }

    fn eval_cond(&mut self, inp: &mut Vec<Token>, d: &Token) -> bool {
        let line = read_line(inp);
        if line.is_empty() {
            err(d, "#if with no expression");
        }
        // Replace `defined X` and `defined(X)` before macro expansion.
        let mut v = vec![];
        let mut i = 0;
        while i < line.len() {
            if line[i].kind == TK::Ident && line[i].s == "defined" {
                let paren = line.get(i + 1).is_some_and(|t| t.is("("));
                let Some(name) = line.get(i + 1 + paren as usize) else { err(d, "macro name missing") };
                v.push(make_token(if self.is_defined(&name.s) { "1" } else { "0" }, &line[i]));
                i += if paren { 4 } else { 2 };
                continue;
            }
            v.push(line[i].clone());
            i += 1;
        }
        let mut toks = self.expand_all(v);
        for t in toks.iter_mut() {
            // Remaining identifiers evaluate to 0.
            if t.kind == TK::Ident {
                t.kind = TK::PPNum;
                t.s = "0".to_string();
            }
        }
        let mut toks = lex::convert(toks);
        for t in toks.iter_mut() {
            // Preprocessor arithmetic is done in (u)intmax_t.
            if t.kind == TK::Num {
                t.nty = match t.nty {
                    NumTy::Int => NumTy::Long,
                    NumTy::UInt => NumTy::ULong,
                    other => other,
                };
            }
        }
        toks.push(Token::new(TK::Eof, "", d.file, d.line));
        crate::parse::const_expr_tokens(toks) != 0
    }

    fn include(&mut self, inp: &mut Vec<Token>, d: &Token) {
        let mut line = read_line(inp);
        let computed = line.first().is_some_and(|t| t.kind == TK::Ident);
        if computed {
            line = self.expand_all(line); // #include MACRO
        }
        let (name, quote) = match line.first() {
            Some(t) if t.kind == TK::Str => (t.s[1..t.s.len() - 1].to_string(), true),
            Some(t) if t.is("<") => {
                let Some(end) = line.iter().position(|t| t.is(">")) else { err(d, "expected '>'") };
                // Like GCC, a name built by macro expansion is glued without spaces.
                (line[1..end].iter().map(|t| if t.space && !computed { format!(" {}", t.s) } else { t.s.clone() }).collect::<String>().trim().to_string(), false)
            }
            _ => err(d, "expected a file name"),
        };
        let cur = file_name(d.file);
        let Some(path) = self.find_include(&name, quote, &cur, d.s == "include_next") else {
            err(d, &format!("{}: cannot find include file", name))
        };
        if self.once.contains(&path) {
            return;
        }
        self.depth += 1;
        if self.depth > 200 {
            err(d, "#include nested too deeply");
        }
        let src = match path.strip_prefix(BUILTIN_DIR) {
            Some(n) => BUILTIN_HEADERS.iter().find(|h| h.0 == &n[1..]).unwrap().1.as_bytes().to_vec(),
            None => std::fs::read(&path).unwrap_or_else(|e| err(d, &format!("{}: {}", path, e))),
        };
        let mut toks = tokenize(&src, add_file(&path));
        // Replace the EOF token with an internal directive that pops the include depth.
        let eof = toks.last_mut().unwrap();
        eof.kind = TK::Punct;
        eof.s = "#".to_string();
        let mut end = Token::new(TK::Ident, "__include_end", eof.file, eof.line);
        end.space = true;
        toks.push(end);
        inp.extend(toks.into_iter().rev());
    }

    fn find_include(&self, name: &str, quote: bool, cur: &str, next: bool) -> Option<String> {
        let exists = |p: &str| match p.strip_prefix(BUILTIN_DIR) {
            Some(n) => BUILTIN_HEADERS.iter().any(|h| h.0 == &n[1..]),
            None => std::path::Path::new(p).is_file(),
        };
        if name.starts_with('/') {
            return exists(name).then(|| name.to_string());
        }
        let cur_dir = match cur.rfind('/') {
            Some(i) => &cur[..i],
            None => ".",
        };
        if quote && !next {
            let p = format!("{}/{}", cur_dir, name);
            if exists(&p) {
                return Some(p);
            }
        }
        let start = if next { self.include_paths.iter().position(|d| d == cur_dir).map_or(0, |i| i + 1) } else { 0 };
        self.include_paths[start..].iter().map(|d| format!("{}/{}", d, name)).find(|p| exists(p))
    }

    fn read_define(&mut self, inp: &mut Vec<Token>) {
        let name = inp.pop().unwrap();
        if name.kind != TK::Ident || name.bol {
            err(&name, "macro name must be an identifier");
        }
        let (mut params, mut va_name, mut objlike) = (vec![], None, true);
        let next = inp.last().unwrap();
        if next.is("(") && !next.space && !next.bol {
            objlike = false;
            inp.pop();
            loop {
                let t = inp.pop().unwrap();
                if t.is(")") && params.is_empty() {
                    break;
                }
                if t.is("...") {
                    va_name = Some("__VA_ARGS__".to_string());
                } else if t.kind == TK::Ident && !t.bol {
                    if inp.last().unwrap().is("...") {
                        inp.pop();
                        va_name = Some(t.s); // GNU: `args...`
                    } else {
                        params.push(t.s);
                    }
                } else {
                    err(&t, "invalid macro parameter list");
                }
                let sep = inp.pop().unwrap();
                if sep.is(")") {
                    break;
                }
                if !sep.is(",") || va_name.is_some() {
                    err(&sep, "expected ',' or ')' in macro parameter list");
                }
            }
        }
        let body = read_line(inp);
        // glibc defines `__attribute__(x)` away for non-GNU compilers; rcc understands
        // attributes (packed and aligned change struct layout), so that is ignored.
        if name.s == "__attribute__" {
            return;
        }
        self.macros.insert(name.s, Rc::new(Macro { objlike, params, va_name, body }));
    }

    /// Fully macro-expands a token sequence (no directives).
    fn expand_all(&mut self, toks: Vec<Token>) -> Vec<Token> {
        let mut inp = toks;
        inp.reverse();
        let mut out = vec![];
        while !inp.is_empty() {
            if !self.expand_macro(&mut inp) {
                out.push(inp.pop().unwrap());
            }
        }
        out
    }

    /// If the next token is a macro invocation, replaces it with its
    /// expansion (pushed back onto the input for rescanning).
    fn expand_macro(&mut self, inp: &mut Vec<Token>) -> bool {
        let n = inp.len();
        let t = &inp[n - 1];
        if t.kind != TK::Ident || t.hide.iter().any(|h| **h == *t.s) {
            return false;
        }
        let Some(m) = self.macros.get(&t.s).cloned() else {
            let text = match t.s.as_str() {
                "__LINE__" => t.line.to_string(),
                "__FILE__" => format!("{:?}", file_name(t.file)),
                "__COUNTER__" => {
                    self.counter += 1;
                    (self.counter - 1).to_string()
                }
                "__BASE_FILE__" => format!("{:?}", self.base_file),
                "__INCLUDE_LEVEL__" => self.depth.to_string(),
                "__TIMESTAMP__" => "\"Thu Jan  1 00:00:00 1970\"".to_string(),
                "__DATE__" => "\"Jan  1 1970\"".to_string(),
                "__TIME__" => "\"00:00:00\"".to_string(),
                _ => return false,
            };
            let new = make_token(&text, t);
            inp[n - 1] = new;
            return true;
        };
        if !m.objlike && !(n >= 2 && inp[n - 2].is("(")) {
            return false;
        }
        let name = inp.pop().unwrap();
        let (mut hs, body) = if m.objlike {
            (name.hide.clone(), m.body.clone())
        } else {
            inp.pop();
            let (args, rparen) = self.read_args(inp, &m, &name);
            let hs: Vec<Rc<str>> = name.hide.iter().filter(|h| rparen.hide.contains(h)).cloned().collect();
            (hs, self.subst(&m, &args))
        };
        hs.push(Rc::from(name.s.as_str()));
        for (i, mut b) in body.into_iter().enumerate().rev() {
            for h in &hs {
                if !b.hide.contains(h) {
                    b.hide.push(h.clone());
                }
            }
            b.file = name.file;
            b.line = name.line;
            b.bol = i == 0 && name.bol;
            if i == 0 {
                b.space = name.space;
            }
            inp.push(b);
        }
        true
    }

    fn read_args(&mut self, inp: &mut Vec<Token>, m: &Macro, name: &Token) -> (Vec<Vec<Token>>, Token) {
        let read_one = |inp: &mut Vec<Token>, rest: bool| {
            let (mut v, mut depth) = (vec![], 0);
            loop {
                let Some(t) = inp.last().filter(|t| t.kind != TK::Eof) else { err(name, "unterminated macro invocation") };
                if depth == 0 && (t.is(")") || (!rest && t.is(","))) {
                    return v;
                }
                if t.is("(") {
                    depth += 1;
                } else if t.is(")") {
                    depth -= 1;
                }
                let mut t = inp.pop().unwrap();
                if t.bol {
                    t.bol = false;
                    t.space = true;
                }
                v.push(t);
            }
        };
        let comma = |inp: &mut Vec<Token>| {
            if !inp.pop().unwrap().is(",") {
                err(name, &format!("too few arguments to macro '{}'", name.s));
            }
        };
        let mut args = vec![];
        for i in 0..m.params.len() {
            if i > 0 {
                comma(inp);
            }
            args.push(read_one(inp, false));
        }
        if m.va_name.is_some() {
            if inp.last().is_some_and(|t| t.is(")")) {
                args.push(vec![]);
            } else {
                if !m.params.is_empty() {
                    comma(inp);
                }
                args.push(read_one(inp, true));
            }
        }
        let rparen = inp.pop().unwrap();
        if !rparen.is(")") {
            err(name, &format!("too many arguments to macro '{}'", name.s));
        }
        (args, rparen)
    }

    fn stringize(&self, hash: &Token, arg: &[Token]) -> Token {
        let mut s = String::new();
        for (i, t) in arg.iter().enumerate() {
            if i > 0 && t.space {
                s.push(' ');
            }
            if t.kind == TK::Str || t.s.contains('\'') {
                s.push_str(&t.s.replace('\\', "\\\\").replace('"', "\\\""));
            } else {
                s.push_str(&t.s);
            }
        }
        make_token(&format!("\"{}\"", s), hash)
    }

    fn paste(&self, l: &Token, r: &Token) -> Token {
        let text = format!("{}{}", l.s, r.s);
        if tokenize(text.as_bytes(), l.file).len() != 2 {
            err(l, &format!("pasting forms '{}', an invalid token", text));
        }
        make_token(&text, l)
    }

    /// Replaces macro parameters in the body with the given arguments.
    fn subst(&mut self, m: &Macro, args: &[Vec<Token>]) -> Vec<Token> {
        let find_arg = |t: &Token| -> Option<usize> {
            if t.kind != TK::Ident {
                return None;
            }
            m.params.iter().position(|p| *p == t.s).or_else(|| (m.va_name.as_deref() == Some(t.s.as_str())).then_some(m.params.len()))
        };
        let body = &m.body;
        let mut out: Vec<Token> = vec![];
        let mut i = 0;
        while i < body.len() {
            let t = &body[i];
            if t.is("#") {
                let Some(a) = body.get(i + 1).and_then(&find_arg) else { err(t, "'#' is not followed by a macro parameter") };
                out.push(self.stringize(t, &args[a]));
                i += 2;
                continue;
            }
            // GNU: `, ## __VA_ARGS__` drops the comma if the variadic argument is empty.
            if t.is(",") && body.get(i + 1).is_some_and(|x| x.is("##")) {
                if let Some(a) = body.get(i + 2).and_then(&find_arg).filter(|a| *a == m.params.len() && m.va_name.is_some()) {
                    if args[a].is_empty() {
                        i += 3;
                    } else {
                        out.push(t.clone());
                        i += 2;
                    }
                    continue;
                }
            }
            if t.is("##") {
                let Some(rhs) = body.get(i + 1) else { err(t, "'##' cannot appear at the end of a macro expansion") };
                if out.is_empty() {
                    err(t, "'##' cannot appear at the start of a macro expansion");
                }
                let last = out.len() - 1;
                match find_arg(rhs) {
                    Some(a) => {
                        if let Some(first) = args[a].first() {
                            out[last] = self.paste(&out[last], first);
                            out.extend(args[a][1..].iter().cloned());
                        }
                    }
                    None => out[last] = self.paste(&out[last], rhs),
                }
                i += 2;
                continue;
            }
            let arg = find_arg(t);
            if let (Some(a), true) = (arg, body.get(i + 1).is_some_and(|x| x.is("##"))) {
                // The left operand of ## is not macro-expanded.
                if args[a].is_empty() {
                    let Some(rhs) = body.get(i + 2) else { err(t, "'##' cannot appear at the end of a macro expansion") };
                    match find_arg(rhs) {
                        Some(b) => out.extend(args[b].iter().cloned()),
                        None => out.push(rhs.clone()),
                    }
                    i += 3;
                } else {
                    out.extend(args[a].iter().cloned());
                    i += 1;
                }
                continue;
            }
            match arg {
                Some(a) => {
                    let mut exp = self.expand_all(args[a].clone());
                    if let Some(first) = exp.first_mut() {
                        first.space = t.space;
                    }
                    out.extend(exp);
                }
                None => out.push(t.clone()),
            }
            i += 1;
        }
        out
    }
}

/// Renders tokens back to text (for -E).
pub fn to_text(toks: &[Token]) -> String {
    let mut s = String::new();
    for (i, t) in toks.iter().enumerate() {
        if t.bol && i > 0 {
            s.push('\n');
        } else if t.space {
            s.push(' ');
        }
        if t.s == "__pragma_pack" {
            s.push_str("#pragma pack");
            continue;
        }
        s.push_str(&t.s);
    }
    s
}
