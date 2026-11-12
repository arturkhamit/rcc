// Recursive-descent parser producing a typed AST. Structure follows chibicc.
use crate::lex::{error_at, NumTy, Token, TK};
use crate::types::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type Loc = (u32, u32); // (file, line)

#[derive(Clone, Copy, PartialEq)]
pub enum Op { Add, Sub, Mul, Div, Mod, BitAnd, BitOr, BitXor, Shl, Shr, Eq, Ne, Lt, Le }

#[derive(Clone)]
pub struct Node {
    pub kind: NK,
    pub ty: Ty,
    pub loc: Loc,
}

#[derive(Clone)]
pub enum NK {
    Nop,
    Num(i64),
    FNum(f64),
    LVar(usize),
    GVar(String),
    Bin(Op, Box<Node>, Box<Node>),
    Neg(Box<Node>),
    Not(Box<Node>),
    BitNot(Box<Node>),
    LogAnd(Box<Node>, Box<Node>),
    LogOr(Box<Node>, Box<Node>),
    Assign(Box<Node>, Box<Node>),
    Cond(Box<Node>, Box<Node>, Box<Node>),
    Comma(Box<Node>, Box<Node>),
    Member(Box<Node>, Member),
    Addr(Box<Node>),
    Deref(Box<Node>),
    Cast(Box<Node>),
    Call { f: Box<Node>, args: Vec<Node>, ret_buf: Option<usize> },
    MemZero(usize),
    VaStart(Box<Node>),
    StmtExpr(Vec<Node>),
    // Statements
    Return(Option<Box<Node>>),
    If(Box<Node>, Box<Node>, Option<Box<Node>>),
    For { init: Option<Box<Node>>, cond: Option<Box<Node>>, inc: Option<Box<Node>>, body: Box<Node>, brk: String, cont: String },
    Do { body: Box<Node>, cond: Box<Node>, brk: String, cont: String },
    Switch { cond: Box<Node>, body: Box<Node>, cases: Vec<(i64, String)>, default: Option<String>, brk: String },
    Label(String, Box<Node>),
    Goto(String),
    Block(Vec<Node>),
    ExprStmt(Box<Node>),
}

pub struct Local {
    pub ty: Ty,
}

pub struct Reloc {
    pub offset: i64,
    pub label: String,
    pub addend: i64,
}

/// A global variable or a function.
pub struct Obj {
    pub name: String,
    pub ty: Ty,
    pub is_static: bool,
    pub is_def: bool,
    pub is_func: bool,
    pub init: Option<Vec<u8>>,
    pub relocs: Vec<Reloc>,
    pub params: Vec<usize>,
    pub locals: Vec<Local>,
    pub body: Option<Node>,
    pub va_area: Option<usize>,
    pub refs: Vec<String>, // globals referenced by the function body
}

#[derive(Clone)]
enum VS {
    Local(usize),
    Global(usize),
    Typedef(Ty),
    Enum(i64),
}

#[derive(Default)]
struct Scope {
    vars: HashMap<String, VS>,
    tags: HashMap<String, Ty>,
}

#[derive(Default)]
struct Attr {
    is_typedef: bool,
    is_static: bool,
    is_extern: bool,
    is_inline: bool,
}

#[derive(Default)]
struct SwitchCtx {
    cases: Vec<(i64, String)>,
    default: Option<String>,
}

struct Init {
    ty: Ty,
    expr: Option<Node>,
    children: Vec<Init>, // arrays: created on demand
    flexible: bool,
    mem: usize, // unions: index of the initialized member
}

fn new_init(ty: &Ty, allow_flex: bool) -> Init {
    let mut init = Init { ty: ty.clone(), expr: None, children: vec![], flexible: false, mem: 0 };
    match &**ty {
        Type::Array(_, n) => init.flexible = allow_flex && *n < 0,
        Type::Struct(sd) => init.children = sd.borrow().members.iter().map(|m| new_init(&m.ty, false)).collect(),
        _ => {}
    }
    init
}

const TYPE_KW: &[&str] = &[
    "void", "_Bool", "char", "short", "int", "long", "struct", "union", "typedef", "enum", "static", "extern",
    "_Alignas", "signed", "unsigned", "const", "volatile", "auto", "register", "restrict", "__restrict",
    "__restrict__", "_Noreturn", "float", "double", "typeof", "__typeof__", "__typeof", "inline", "__inline",
    "__inline__", "_Thread_local", "__thread", "_Atomic", "__attribute__", "__signed__", "__signed", "__const",
    "__volatile__", "__volatile",
];

fn err(loc: Loc, msg: &str) -> ! {
    error_at(loc.0, loc.1, msg)
}

fn mk(kind: NK, ty: Ty, loc: Loc) -> Node {
    Node { kind, ty, loc }
}

fn bx(n: Node) -> Box<Node> {
    Box::new(n)
}

fn num(v: i64, loc: Loc) -> Node {
    mk(NK::Num(v), ty_int(), loc)
}

fn lnum(v: i64, loc: Loc) -> Node {
    mk(NK::Num(v), ty_long(), loc)
}

/// Truncates a constant to the width of an integer type.
fn wrap(v: i64, ty: &Type) -> i64 {
    match ty {
        Type::Bool => (v != 0) as i64,
        Type::Int { size: 1, unsigned: false } => v as i8 as i64,
        Type::Int { size: 1, unsigned: true } => v as u8 as i64,
        Type::Int { size: 2, unsigned: false } => v as i16 as i64,
        Type::Int { size: 2, unsigned: true } => v as u16 as i64,
        Type::Int { size: 4, unsigned: false } => v as i32 as i64,
        Type::Int { size: 4, unsigned: true } => v as u32 as i64,
        _ => v,
    }
}

pub fn new_cast(e: Node, ty: Ty) -> Node {
    // Fold casts of integer constants.
    if let (NK::Num(v), true) = (&e.kind, e.ty.is_integer()) {
        let uv = if e.ty.is_unsigned() && e.ty.size() == 8 { *v as u64 as f64 } else { *v as f64 };
        match &*ty {
            Type::Int { .. } => return mk(NK::Num(wrap(*v, &ty)), ty, e.loc),
            Type::Double => return mk(NK::FNum(uv), ty, e.loc),
            Type::Float => return mk(NK::FNum(uv as f32 as f64), ty, e.loc),
            _ => {}
        }
    }
    let same = match (&*e.ty, &*ty) {
        (Type::Int { size: a, unsigned: b }, Type::Int { size: c, unsigned: d }) => a == c && b == d,
        (Type::Bool, Type::Bool) | (Type::Float, Type::Float) | (Type::Double, Type::Double) => true,
        _ => false,
    };
    if same {
        return e;
    }
    let loc = e.loc;
    mk(NK::Cast(bx(e)), ty, loc)
}

fn arith(op: Op, l: Node, r: Node, loc: Loc) -> Node {
    if !l.ty.is_numeric() && l.ty.base().is_none() && !matches!(*l.ty, Type::Func(_)) || r.ty.is_struct() {
        err(loc, "invalid operands");
    }
    let ty = common_type(&l.ty, &r.ty);
    let rty = if matches!(op, Op::Eq | Op::Ne | Op::Lt | Op::Le) { ty_int() } else { ty.clone() };
    mk(NK::Bin(op, bx(new_cast(l, ty.clone())), bx(new_cast(r, ty))), rty, loc)
}

fn scale(idx: Node, size: i64, loc: Loc) -> Node {
    let idx = new_cast(idx, ty_long());
    if let NK::Num(v) = idx.kind {
        return lnum(v.wrapping_mul(size), loc);
    }
    mk(NK::Bin(Op::Mul, bx(idx), bx(lnum(size, loc))), ty_long(), loc)
}

fn new_add(l: Node, r: Node, loc: Loc) -> Node {
    if l.ty.is_numeric() && r.ty.is_numeric() {
        return arith(Op::Add, l, r, loc);
    }
    let (l, r) = if l.ty.base().is_none() && r.ty.base().is_some() { (r, l) } else { (l, r) };
    let Some(base) = l.ty.base().cloned() else { err(loc, "invalid operands") };
    if !r.ty.is_integer() {
        err(loc, "invalid operands");
    }
    let r = scale(r, base.size(), loc);
    mk(NK::Bin(Op::Add, bx(l), bx(r)), ptr_to(base), loc)
}

fn new_sub(l: Node, r: Node, loc: Loc) -> Node {
    if l.ty.is_numeric() && r.ty.is_numeric() {
        return arith(Op::Sub, l, r, loc);
    }
    let Some(base) = l.ty.base().cloned() else { err(loc, "invalid operands") };
    if r.ty.is_integer() {
        let r = scale(r, base.size(), loc);
        return mk(NK::Bin(Op::Sub, bx(l), bx(r)), ptr_to(base), loc);
    }
    if r.ty.base().is_none() {
        err(loc, "invalid operands");
    }
    let diff = mk(NK::Bin(Op::Sub, bx(l), bx(r)), ty_long(), loc);
    mk(NK::Bin(Op::Div, bx(diff), bx(lnum(base.size().max(1), loc))), ty_long(), loc)
}

fn new_binary(op: Op, l: Node, r: Node, loc: Loc) -> Node {
    match op {
        Op::Add => new_add(l, r, loc),
        Op::Sub => new_sub(l, r, loc),
        Op::Shl | Op::Shr => {
            let ty = promote(&l.ty);
            mk(NK::Bin(op, bx(new_cast(l, ty.clone())), bx(r)), ty, loc)
        }
        _ => arith(op, l, r, loc),
    }
}

fn new_deref(n: Node, loc: Loc) -> Node {
    if let Type::Func(_) = &*n.ty {
        return n;
    }
    let Some(base) = n.ty.base().cloned() else { err(loc, "invalid pointer dereference") };
    mk(NK::Deref(bx(n)), base, loc)
}

fn new_addr(n: Node, loc: Loc) -> Node {
    let ty = ptr_to(n.ty.clone());
    mk(NK::Addr(bx(n)), ty, loc)
}

fn new_assign(l: Node, r: Node, loc: Loc) -> Node {
    if let Type::Array(..) = &*l.ty {
        err(loc, "not an lvalue");
    }
    let r = if l.ty.is_struct() { r } else { new_cast(r, l.ty.clone()) };
    let ty = l.ty.clone();
    mk(NK::Assign(bx(l), bx(r)), ty, loc)
}

fn new_comma(l: Node, r: Node, loc: Loc) -> Node {
    let ty = r.ty.clone();
    mk(NK::Comma(bx(l), bx(r)), ty, loc)
}

fn new_member(n: Node, m: &Member, loc: Loc) -> Node {
    mk(NK::Member(bx(n), m.clone()), m.ty.clone(), loc)
}

// ---------------------------------------------------------------------------
// Constant expression evaluation

pub fn eval(n: &Node) -> i64 {
    eval2(n, None)
}

/// Evaluates a constant expression. If `label` is given, the result may be an
/// address constant `label + value`.
fn eval2(n: &Node, mut label: Option<&mut String>) -> i64 {
    if n.ty.is_flonum() {
        return eval_double(n) as i64;
    }
    let v = match &n.kind {
        NK::Bin(op, l, r) => {
            let uns = l.ty.is_unsigned();
            let wide = l.ty.size() == 8;
            match op {
                Op::Add => eval2(l, label).wrapping_add(eval(r)),
                Op::Sub => eval2(l, label).wrapping_sub(eval(r)),
                _ => {
                    let (a, b) = (eval(l), eval(r));
                    if matches!(op, Op::Div | Op::Mod) && b == 0 {
                        err(n.loc, "division by zero in constant expression");
                    }
                    match op {
                        Op::Mul => a.wrapping_mul(b),
                        Op::Div => if uns { ((a as u64) / (b as u64)) as i64 } else { a.wrapping_div(b) },
                        Op::Mod => if uns { ((a as u64) % (b as u64)) as i64 } else { a.wrapping_rem(b) },
                        Op::BitAnd => a & b,
                        Op::BitOr => a | b,
                        Op::BitXor => a ^ b,
                        Op::Shl => a.wrapping_shl(b as u32),
                        Op::Shr if !uns => a >> (b & 63),
                        Op::Shr if wide => ((a as u64) >> (b & 63)) as i64,
                        Op::Shr => ((a as u32) >> (b & 31)) as i64,
                        Op::Eq => (a == b) as i64,
                        Op::Ne => (a != b) as i64,
                        Op::Lt => if uns { ((a as u64) < (b as u64)) as i64 } else { (a < b) as i64 },
                        Op::Le => if uns { ((a as u64) <= (b as u64)) as i64 } else { (a <= b) as i64 },
                        _ => unreachable!(),
                    }
                }
            }
        }
        NK::Cond(c, t, e) => if eval_bool(c) { eval2(t, label) } else { eval2(e, label) },
        NK::Comma(_, r) => eval2(r, label),
        NK::Not(l) => !eval_bool(l) as i64,
        NK::BitNot(l) => !eval(l),
        NK::Neg(l) => eval(l).wrapping_neg(),
        NK::LogAnd(l, r) => (eval_bool(l) && eval_bool(r)) as i64,
        NK::LogOr(l, r) => (eval_bool(l) || eval_bool(r)) as i64,
        NK::Cast(l) if l.ty.is_flonum() => {
            let d = eval_double(l);
            match &*n.ty {
                Type::Bool => (d != 0.0) as i64,
                Type::Int { size: 8, unsigned: true } => d as u64 as i64,
                _ => d as i64,
            }
        }
        NK::Cast(l) => eval2(l, label),
        NK::Addr(l) => eval_rval(l, label),
        NK::Member(l, m) => {
            if label.is_none() || !matches!(&*n.ty, Type::Array(..)) {
                err(n.loc, "not a compile-time constant");
            }
            eval_rval(l, label) + m.offset
        }
        NK::GVar(name) => {
            if !matches!(&*n.ty, Type::Array(..) | Type::Func(_)) {
                err(n.loc, "not a compile-time constant");
            }
            match label.as_deref_mut() {
                Some(l) => *l = name.clone(),
                None => err(n.loc, "not a compile-time constant"),
            }
            0
        }
        NK::Num(v) => *v,
        _ => err(n.loc, "not a compile-time constant"),
    };
    wrap(v, &n.ty)
}

fn eval_bool(n: &Node) -> bool {
    if n.ty.is_flonum() { eval_double(n) != 0.0 } else { eval(n) != 0 }
}

fn eval_rval(n: &Node, label: Option<&mut String>) -> i64 {
    match &n.kind {
        NK::GVar(name) => {
            match label {
                Some(l) => *l = name.clone(),
                None => err(n.loc, "not a compile-time constant"),
            }
            0
        }
        NK::Deref(l) => eval2(l, label),
        NK::Member(l, m) => eval_rval(l, label) + m.offset,
        _ => err(n.loc, "invalid initializer"),
    }
}

fn eval_double(n: &Node) -> f64 {
    if !n.ty.is_flonum() {
        let v = eval(n);
        return if n.ty.is_unsigned() && n.ty.size() == 8 { v as u64 as f64 } else { v as f64 };
    }
    let v = match &n.kind {
        NK::Bin(op, l, r) => {
            let (a, b) = (eval_double(l), eval_double(r));
            match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                _ => err(n.loc, "not a compile-time constant"),
            }
        }
        NK::Neg(l) => -eval_double(l),
        NK::Cond(c, t, e) => if eval_bool(c) { eval_double(t) } else { eval_double(e) },
        NK::Comma(_, r) => eval_double(r),
        NK::Cast(l) => eval_double(l),
        NK::FNum(v) => *v,
        _ => err(n.loc, "not a compile-time constant"),
    };
    if let Type::Float = &*n.ty { v as f32 as f64 } else { v }
}

// ---------------------------------------------------------------------------

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    scopes: Vec<Scope>,
    pub globals: Vec<Obj>,
    gmap: HashMap<String, usize>,
    locals: Vec<Local>,
    refs: Vec<String>,
    fn_name: String,
    ret_ty: Ty,
    brk: Option<String>,
    cont: Option<String>,
    switches: Vec<SwitchCtx>,
    uniq: usize,
    va_elem: Ty,
}

pub fn parse(toks: Vec<Token>) -> Vec<Obj> {
    // struct __va_elem as defined by the System V x86-64 ABI
    let mem = |name: &str, ty: Ty, offset: i64| Member { name: name.into(), ty, offset, bit_width: -1, bit_off: 0 };
    let members = vec![
        mem("gp_offset", ty_uint(), 0),
        mem("fp_offset", ty_uint(), 4),
        mem("overflow_arg_area", ptr_to(ty_char()), 8),
        mem("reg_save_area", ptr_to(ty_char()), 16),
    ];
    let va_elem: Ty = Rc::new(Type::Struct(Rc::new(RefCell::new(SDef { is_union: false, members, size: 24, align: 8, complete: true }))));
    let mut p = Parser {
        toks,
        pos: 0,
        scopes: vec![Scope::default()],
        globals: vec![],
        gmap: HashMap::new(),
        locals: vec![],
        refs: vec![],
        fn_name: String::new(),
        ret_ty: ty_void(),
        brk: None,
        cont: None,
        switches: vec![],
        uniq: 0,
        va_elem: va_elem.clone(),
    };
    p.scopes[0].vars.insert("__builtin_va_list".into(), VS::Typedef(array_of(va_elem, 1)));
    p.program();
    p.globals
}

impl Parser {
    // ----- token helpers -----

    fn tok(&self) -> &Token {
        &self.toks[self.pos]
    }
    fn loc(&self) -> Loc {
        let t = self.tok();
        (t.file, t.line)
    }
    fn err(&self, msg: &str) -> ! {
        err(self.loc(), msg)
    }
    fn is(&self, s: &str) -> bool {
        self.tok().is(s)
    }
    fn is_at(&self, off: usize, s: &str) -> bool {
        self.toks.get(self.pos + off).is_some_and(|t| t.is(s))
    }
    fn consume(&mut self, s: &str) -> bool {
        if self.is(s) {
            self.pos += 1;
            return true;
        }
        false
    }
    fn expect(&mut self, s: &str) {
        if !self.consume(s) {
            self.err(&format!("expected '{}' before '{}'", s, self.tok().s));
        }
    }
    fn ident(&mut self) -> String {
        if self.tok().kind != TK::Ident {
            self.err(&format!("expected an identifier before '{}'", self.tok().s));
        }
        self.pos += 1;
        self.toks[self.pos - 1].s.clone()
    }
    fn label(&mut self) -> String {
        self.uniq += 1;
        format!(".L.p.{}", self.uniq)
    }
    /// Skips `__attribute__((...))` and `__asm__("...")`.
    fn skip_attrs(&mut self) {
        while self.is("__attribute__") || self.is("__asm__") || self.is("__asm") {
            self.pos += 1;
            self.expect("(");
            let mut depth = 1;
            while depth > 0 {
                if self.tok().kind == TK::Eof {
                    self.err("unterminated attribute");
                }
                if self.is("(") {
                    depth += 1;
                } else if self.is(")") {
                    depth -= 1;
                }
                self.pos += 1;
            }
        }
    }
    fn skip_to_semi(&mut self) {
        while !self.consume(";") {
            if self.tok().kind == TK::Eof {
                self.err("expected ';'");
            }
            self.pos += 1;
        }
    }

    // ----- scopes -----

    fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().unwrap()
    }
    fn find_var(&self, name: &str) -> Option<&VS> {
        self.scopes.iter().rev().find_map(|s| s.vars.get(name))
    }
    fn find_tag(&self, name: &str) -> Option<Ty> {
        self.scopes.iter().rev().find_map(|s| s.tags.get(name)).cloned()
    }
    fn new_lvar(&mut self, name: &str, ty: Ty) -> usize {
        self.locals.push(Local { ty });
        let i = self.locals.len() - 1;
        if !name.is_empty() {
            self.scope().vars.insert(name.to_string(), VS::Local(i));
        }
        i
    }
    fn lvar(&self, i: usize, loc: Loc) -> Node {
        mk(NK::LVar(i), self.locals[i].ty.clone(), loc)
    }
    fn new_global(&mut self, name: String, ty: Ty) -> usize {
        self.globals.push(Obj {
            name: name.clone(),
            ty,
            is_static: false,
            is_def: false,
            is_func: false,
            init: None,
            relocs: vec![],
            params: vec![],
            locals: vec![],
            body: None,
            va_area: None,
            refs: vec![],
        });
        self.gmap.insert(name, self.globals.len() - 1);
        self.globals.len() - 1
    }
    fn get_or_new_global(&mut self, name: &str, ty: Ty) -> usize {
        if let Some(&i) = self.gmap.get(name) {
            let update = match (&*self.globals[i].ty, &*ty) {
                (Type::Array(_, n), Type::Array(_, m)) => *n < 0 && *m >= 0,
                (Type::Func(a), Type::Func(b)) => a.oldstyle && !b.oldstyle,
                _ => false,
            };
            if update {
                self.globals[i].ty = ty;
            }
            return i;
        }
        self.new_global(name.to_string(), ty)
    }
    fn new_anon_global(&mut self, prefix: &str, ty: Ty) -> usize {
        self.uniq += 1;
        let i = self.new_global(format!("{}.{}", prefix, self.uniq), ty);
        self.globals[i].is_static = true;
        self.globals[i].is_def = true;
        i
    }
    fn gvar(&mut self, i: usize, loc: Loc) -> Node {
        let g = &self.globals[i];
        self.refs.push(g.name.clone());
        mk(NK::GVar(g.name.clone()), g.ty.clone(), loc)
    }
    fn new_string(&mut self, mut bytes: Vec<u8>, elem: i64) -> usize {
        bytes.extend(std::iter::repeat_n(0u8, elem as usize));
        let ety = if elem == 1 { ty_char() } else { ty_int() };
        let i = self.new_anon_global(".L.str", array_of(ety, bytes.len() as i64 / elem));
        self.globals[i].init = Some(bytes);
        i
    }

    // ----- types and declarators -----

    fn is_typename_at(&self, i: usize) -> bool {
        let t = &self.toks[i];
        match t.kind {
            TK::Keyword => TYPE_KW.contains(&t.s.as_str()),
            TK::Ident => matches!(self.find_var(&t.s), Some(VS::Typedef(_))),
            _ => false,
        }
    }
    fn is_typename(&self) -> bool {
        self.is_typename_at(self.pos)
    }

    fn declspec(&mut self, mut attr: Option<&mut Attr>) -> Ty {
        const VOID: u32 = 1 << 0;
        const BOOL: u32 = 1 << 2;
        const CHAR: u32 = 1 << 4;
        const SHORT: u32 = 1 << 6;
        const INT: u32 = 1 << 8;
        const LONG: u32 = 1 << 10;
        const FLOAT: u32 = 1 << 12;
        const DOUBLE: u32 = 1 << 14;
        const OTHER: u32 = 1 << 16;
        const SIGNED: u32 = 1 << 17;
        const UNSIGNED: u32 = 1 << 18;

        let mut ty = ty_int();
        let mut counter = 0u32;
        while self.is_typename() {
            let s = self.tok().s.clone();
            match s.as_str() {
                "typedef" | "static" | "extern" | "inline" | "__inline" | "__inline__" | "_Thread_local" | "__thread" => {
                    let Some(a) = attr.as_deref_mut() else { self.err("storage class specifier is not allowed in this context") };
                    match s.as_str() {
                        "typedef" => a.is_typedef = true,
                        "static" => a.is_static = true,
                        "extern" => a.is_extern = true,
                        "_Thread_local" | "__thread" => self.err("thread-local storage is not supported"),
                        _ => a.is_inline = true,
                    }
                    self.pos += 1;
                    continue;
                }
                "const" | "volatile" | "auto" | "register" | "restrict" | "__restrict" | "__restrict__" | "_Noreturn"
                | "__const" | "__volatile__" | "__volatile" | "_Atomic" => {
                    self.pos += 1;
                    continue;
                }
                "__attribute__" => {
                    self.skip_attrs();
                    continue;
                }
                "_Alignas" => {
                    // shortcut: _Alignas is parsed and ignored; honor it if over-aligned data is needed.
                    self.pos += 1;
                    self.expect("(");
                    if self.is_typename() {
                        self.typename();
                    } else {
                        self.const_expr();
                    }
                    self.expect(")");
                    continue;
                }
                _ => {}
            }
            let td = match self.find_var(&s) {
                Some(VS::Typedef(t)) if self.tok().kind == TK::Ident => Some(t.clone()),
                _ => None,
            };
            if matches!(s.as_str(), "struct" | "union" | "enum" | "typeof" | "__typeof__" | "__typeof") || td.is_some() {
                if counter != 0 {
                    break;
                }
                self.pos += 1;
                ty = match s.as_str() {
                    "struct" => self.struct_decl(false),
                    "union" => self.struct_decl(true),
                    "enum" => self.enum_spec(),
                    "typeof" | "__typeof__" | "__typeof" => {
                        self.expect("(");
                        let t = if self.is_typename() { self.typename() } else { self.expr().ty };
                        self.expect(")");
                        t
                    }
                    _ => td.unwrap(),
                };
                counter += OTHER;
                continue;
            }
            match s.as_str() {
                "void" => counter += VOID,
                "_Bool" => counter += BOOL,
                "char" => counter += CHAR,
                "short" => counter += SHORT,
                "int" => counter += INT,
                "long" => counter += LONG,
                "float" => counter += FLOAT,
                "double" => counter += DOUBLE,
                "signed" | "__signed__" | "__signed" => counter |= SIGNED,
                "unsigned" => counter |= UNSIGNED,
                _ => self.err("invalid type"),
            }
            self.pos += 1;
            let c = counter & !SIGNED;
            let u = c & UNSIGNED != 0;
            let b = c & !UNSIGNED;
            ty = if c == VOID {
                ty_void()
            } else if c == BOOL {
                ty_bool()
            } else if b == CHAR {
                ty_ints(1, u)
            } else if b == SHORT || b == SHORT + INT {
                ty_ints(2, u)
            } else if b == INT || b == 0 {
                ty_ints(4, u)
            } else if b == LONG || b == LONG + INT || b == LONG + LONG || b == LONG + LONG + INT {
                ty_ints(8, u)
            } else if c == FLOAT {
                ty_float()
            } else if c == DOUBLE || c == LONG + DOUBLE {
                ty_double()
            } else {
                self.err("invalid type")
            };
        }
        ty
    }

    fn pointers(&mut self, mut ty: Ty) -> Ty {
        loop {
            self.skip_attrs();
            if self.consume("*") {
                ty = ptr_to(ty);
            } else if ["const", "volatile", "restrict", "__restrict", "__restrict__", "__const", "_Atomic"].iter().any(|k| self.is(k)) {
                self.pos += 1;
            } else {
                return ty;
            }
        }
    }

    /// Parses a (possibly abstract) declarator. Returns the type and the name.
    fn declarator(&mut self, ty: Ty) -> (Ty, Option<String>) {
        let ty = self.pointers(ty);
        if self.is("(") && !self.is_typename_at(self.pos + 1) && !self.is_at(1, ")") {
            // Nested declarator: parse the suffix first, then the inner part.
            let start = self.pos + 1;
            self.pos = start;
            self.declarator(ty_int());
            self.expect(")");
            let ty = self.type_suffix(ty);
            let end = self.pos;
            self.pos = start;
            let r = self.declarator(ty);
            self.pos = end;
            return r;
        }
        let mut name = None;
        if self.tok().kind == TK::Ident {
            name = Some(self.ident());
        }
        let ty = self.type_suffix(ty);
        (ty, name)
    }

    fn type_suffix(&mut self, ty: Ty) -> Ty {
        if self.consume("(") {
            return self.func_params(ty);
        }
        if self.consume("[") {
            while self.is("static") || self.is("restrict") || self.is("const") || self.is("volatile") {
                self.pos += 1;
            }
            let len = if self.is("]") { -1 } else { self.const_expr() };
            self.expect("]");
            let ty = self.type_suffix(ty);
            return array_of(ty, len);
        }
        ty
    }

    fn func_params(&mut self, ret: Ty) -> Ty {
        let (mut params, mut names, mut variadic) = (vec![], vec![], false);
        if self.is("void") && self.is_at(1, ")") {
            self.pos += 2;
        } else {
            let oldstyle = self.is(")");
            while !self.consume(")") {
                if !params.is_empty() {
                    self.expect(",");
                }
                if self.consume("...") {
                    variadic = true;
                    self.expect(")");
                    break;
                }
                let base = self.declspec(None);
                let (ty, name) = self.declarator(base);
                self.skip_attrs();
                params.push(decay(&ty));
                names.push(name.unwrap_or_default());
            }
            if oldstyle {
                return Rc::new(Type::Func(Rc::new(FuncTy { ret, params, names, variadic: false, oldstyle: true })));
            }
        }
        Rc::new(Type::Func(Rc::new(FuncTy { ret, params, names, variadic, oldstyle: false })))
    }

    fn typename(&mut self) -> Ty {
        let base = self.declspec(None);
        self.declarator(base).0
    }

    fn struct_decl(&mut self, is_union: bool) -> Ty {
        self.skip_attrs();
        let mut tag = None;
        if self.tok().kind == TK::Ident {
            tag = Some(self.ident());
        }
        if !self.is("{") {
            let Some(tag) = tag else { self.err("expected a struct tag or '{'") };
            if let Some(t) = self.find_tag(&tag) {
                return t;
            }
            let t = new_struct(is_union);
            self.scope().tags.insert(tag, t.clone());
            return t;
        }
        self.pos += 1;
        // Complete a forward-declared struct in the same scope, or make a new one.
        let existing = tag.as_ref().and_then(|t| self.scope().tags.get(t).cloned());
        let ty = match existing {
            Some(t) if matches!(&*t, Type::Struct(sd) if !sd.borrow().complete) => t,
            _ => {
                let t = new_struct(is_union);
                if let Some(tag) = &tag {
                    self.scope().tags.insert(tag.clone(), t.clone());
                }
                t
            }
        };

        let mut members: Vec<Member> = Vec::new();
        while !self.consume("}") {
            let mut attr = Attr::default();
            let base = self.declspec(Some(&mut attr));
            if base.is_struct() && self.consume(";") {
                members.push(Member { name: String::new(), ty: base, offset: 0, bit_width: -1, bit_off: 0 });
                continue;
            }
            let mut first = true;
            while !self.consume(";") {
                if !first {
                    self.expect(",");
                }
                first = false;
                let (mty, name) = if self.is(":") { (base.clone(), None) } else { self.declarator(base.clone()) };
                let mut m = Member { name: name.unwrap_or_default(), ty: mty, offset: 0, bit_width: -1, bit_off: 0 };
                if self.consume(":") {
                    m.bit_width = self.const_expr();
                }
                self.skip_attrs();
                members.push(m);
            }
        }
        self.skip_attrs();

        // Layout (bitfields follow the GCC/System V rules).
        let (mut bits, mut align, mut size) = (0i64, 1i64, 0i64);
        for m in members.iter_mut() {
            let (msz, mal) = (m.ty.size(), m.ty.align());
            if is_union {
                size = size.max(msz);
                align = align.max(mal);
                continue;
            }
            if m.bit_width == 0 {
                bits = align_to(bits, msz * 8);
                continue;
            }
            if m.bit_width > 0 {
                if bits / (msz * 8) != (bits + m.bit_width - 1) / (msz * 8) {
                    bits = align_to(bits, msz * 8);
                }
                m.offset = bits / (msz * 8) * msz;
                m.bit_off = bits % (msz * 8);
                bits += m.bit_width;
            } else {
                bits = align_to(bits, mal * 8);
                m.offset = bits / 8;
                bits += msz * 8;
            }
            align = align.max(mal);
        }
        if !is_union {
            size = align_to(bits, 8) / 8;
        }
        // Unnamed bitfields only affect layout.
        members.retain(|m| !(m.name.is_empty() && m.bit_width >= 0));
        let Type::Struct(sd) = &*ty else { unreachable!() };
        let mut sd = sd.borrow_mut();
        sd.is_union = is_union;
        sd.members = members;
        sd.size = align_to(size, align);
        sd.align = align;
        sd.complete = true;
        drop(sd);
        ty
    }

    fn enum_spec(&mut self) -> Ty {
        self.skip_attrs();
        if self.tok().kind == TK::Ident {
            self.pos += 1;
        }
        if !self.consume("{") {
            return ty_int();
        }
        let mut val = 0i64;
        while !self.consume("}") {
            let name = self.ident();
            self.skip_attrs();
            if self.consume("=") {
                val = self.const_expr();
            }
            self.scope().vars.insert(name, VS::Enum(val));
            val += 1;
            if !self.consume(",") {
                self.expect("}");
                break;
            }
        }
        ty_int()
    }

    fn parse_typedef(&mut self, base: Ty) {
        let mut first = true;
        while !self.consume(";") {
            if !first {
                self.expect(",");
            }
            first = false;
            let (ty, name) = self.declarator(base.clone());
            self.skip_attrs();
            let Some(name) = name else { self.err("typedef name omitted") };
            self.scope().vars.insert(name, VS::Typedef(ty));
        }
    }

    // ----- top level -----

    fn program(&mut self) {
        while self.tok().kind != TK::Eof {
            if self.consume(";") {
                continue;
            }
            if self.is("_Static_assert") || self.is("__asm__") || self.is("__asm") {
                self.skip_to_semi();
                continue;
            }
            let mut attr = Attr::default();
            let base = self.declspec(Some(&mut attr));
            if attr.is_typedef {
                self.parse_typedef(base);
                continue;
            }
            let mut first = true;
            while !self.consume(";") {
                if !first {
                    self.expect(",");
                }
                first = false;
                let (ty, name) = self.declarator(base.clone());
                self.skip_attrs();
                let Some(name) = name else { self.err("variable name omitted") };
                if let Type::Func(_) = &*ty {
                    if self.is("{") {
                        self.function_def(ty, &name, &attr);
                        break;
                    }
                    let i = self.declare_func(&name, ty, &attr);
                    self.scopes[0].vars.insert(name, VS::Global(i));
                    continue;
                }
                let i = self.get_or_new_global(&name, ty);
                self.scopes[0].vars.insert(name, VS::Global(i));
                self.globals[i].is_static |= attr.is_static;
                if !attr.is_extern {
                    self.globals[i].is_def = true;
                }
                if self.consume("=") {
                    self.gvar_initializer(i);
                }
            }
        }
    }

    fn declare_func(&mut self, name: &str, ty: Ty, attr: &Attr) -> usize {
        let i = self.get_or_new_global(name, ty);
        self.globals[i].is_func = true;
        self.globals[i].is_static |= attr.is_static;
        i
    }

    fn function_def(&mut self, ty: Ty, name: &str, attr: &Attr) {
        let Type::Func(fty) = &*ty else { unreachable!() };
        let fty = fty.clone();
        let i = self.declare_func(name, ty.clone(), attr);
        self.scopes[0].vars.insert(name.to_string(), VS::Global(i));
        let g = &mut self.globals[i];
        g.ty = ty.clone();
        g.is_static |= attr.is_inline && !attr.is_extern;
        g.is_def = true;

        self.locals = vec![];
        self.refs = vec![];
        self.fn_name = name.to_string();
        self.ret_ty = fty.ret.clone();
        self.scopes.push(Scope::default());
        let mut params = vec![];
        if fty.ret.is_struct() && fty.ret.size() > 16 {
            // Hidden pointer to the caller's return buffer.
            params.push(self.new_lvar("", ptr_to(fty.ret.clone())));
        }
        for (pty, pname) in fty.params.iter().zip(&fty.names) {
            params.push(self.new_lvar(pname, pty.clone()));
        }
        let va_area = if fty.variadic { Some(self.new_lvar("", array_of(ty_char(), 176))) } else { None };
        self.expect("{");
        let body = self.block();
        self.scopes.pop();

        let g = &mut self.globals[i];
        g.params = params;
        g.va_area = va_area;
        g.locals = std::mem::take(&mut self.locals);
        g.refs = std::mem::take(&mut self.refs);
        g.body = Some(body);
    }

    // ----- initializers -----

    fn is_end(&self) -> bool {
        self.is("}") || (self.is(",") && self.is_at(1, "}"))
    }
    fn consume_end(&mut self) -> bool {
        if self.consume("}") {
            return true;
        }
        if self.is(",") && self.is_at(1, "}") {
            self.pos += 2;
            return true;
        }
        false
    }
    fn skip_excess(&mut self) {
        if self.consume("{") {
            self.skip_excess();
            self.expect("}");
        } else {
            self.assign();
        }
    }
    fn ensure_child(&mut self, init: &mut Init, idx: usize) {
        let Type::Array(base, len) = &*init.ty else { unreachable!() };
        if !init.flexible && idx as i64 >= *len {
            self.err("array index in initializer exceeds array bounds");
        }
        while init.children.len() <= idx {
            init.children.push(new_init(base, false));
        }
    }

    fn string_init(&mut self, init: &mut Init) {
        let t = self.tok().clone();
        let loc = self.loc();
        self.pos += 1;
        let Type::Array(base, len) = &*init.ty.clone() else { unreachable!() };
        let esz = t.ival as usize;
        let n = t.bytes.len() / esz;
        let total = if init.flexible { n + 1 } else { (*len as usize).min(n + 1) };
        init.children.clear();
        for i in 0..total {
            let v = if i >= n {
                0
            } else if esz == 1 {
                t.bytes[i] as i8 as i64
            } else {
                i32::from_le_bytes(t.bytes[i * 4..i * 4 + 4].try_into().unwrap()) as i64
            };
            let mut c = new_init(base, false);
            c.expr = Some(mk(NK::Num(wrap(v, base)), base.clone(), loc));
            init.children.push(c);
        }
    }

    fn struct_designator(&mut self, ty: &Ty) -> usize {
        let name = self.toks[self.pos + 1].s.clone();
        let Type::Struct(sd) = &**ty else { unreachable!() };
        for (i, m) in sd.borrow().members.iter().enumerate() {
            if m.name.is_empty() {
                // Leave `.name` for the anonymous member's own designation.
                if find_member(&m.ty, &name).is_some() {
                    return i;
                }
            } else if m.name == name {
                self.pos += 2;
                return i;
            }
        }
        self.err("struct has no such member")
    }

    fn designation(&mut self, init: &mut Init) {
        if self.is("[") {
            if !matches!(&*init.ty, Type::Array(..)) {
                self.err("array index in non-array initializer");
            }
            self.pos += 1;
            let idx = self.const_expr() as usize;
            self.expect("]");
            self.ensure_child(init, idx);
            self.designation(&mut init.children[idx]);
            self.array_init2(init, idx + 1);
            return;
        }
        if self.is(".") {
            let Type::Struct(sd) = &*init.ty else { self.err("field name not in struct or union initializer") };
            let is_union = sd.borrow().is_union;
            let i = self.struct_designator(&init.ty.clone());
            self.designation(&mut init.children[i]);
            init.expr = None;
            if is_union {
                init.mem = i;
            } else {
                self.struct_init2(init, i + 1, true);
            }
            return;
        }
        self.consume("=");
        self.initializer2(init);
    }

    fn array_init1(&mut self, init: &mut Init) {
        self.expect("{");
        let Type::Array(_, len) = &*init.ty else { unreachable!() };
        let len = *len;
        let (mut i, mut first) = (0usize, true);
        while !self.consume_end() {
            if !first {
                self.expect(",");
            }
            first = false;
            if self.is("[") {
                self.pos += 1;
                let idx = self.const_expr() as usize;
                self.expect("]");
                self.ensure_child(init, idx);
                self.designation(&mut init.children[idx]);
                i = idx + 1;
                continue;
            }
            if init.flexible || (i as i64) < len {
                self.ensure_child(init, i);
                self.initializer2(&mut init.children[i]);
            } else {
                self.skip_excess();
            }
            i += 1;
        }
    }

    fn array_init2(&mut self, init: &mut Init, mut i: usize) {
        let Type::Array(_, len) = &*init.ty else { unreachable!() };
        let len = *len;
        while (init.flexible || (i as i64) < len) && !self.is_end() {
            let save = self.pos;
            if i > 0 {
                self.expect(",");
            }
            if self.is("[") || self.is(".") {
                self.pos = save;
                return;
            }
            self.ensure_child(init, i);
            self.initializer2(&mut init.children[i]);
            i += 1;
        }
    }

    fn struct_init1(&mut self, init: &mut Init) {
        self.expect("{");
        let (mut i, mut first) = (0usize, true);
        while !self.consume_end() {
            if !first {
                self.expect(",");
            }
            first = false;
            if self.is(".") {
                i = self.struct_designator(&init.ty.clone());
                self.designation(&mut init.children[i]);
                i += 1;
                continue;
            }
            if i < init.children.len() {
                self.initializer2(&mut init.children[i]);
                i += 1;
            } else {
                self.skip_excess();
            }
        }
    }

    fn struct_init2(&mut self, init: &mut Init, mut i: usize, post_desig: bool) {
        let mut first = true;
        while i < init.children.len() && !self.is_end() {
            let save = self.pos;
            if !first || post_desig {
                self.expect(",");
            }
            first = false;
            if self.is("[") || self.is(".") {
                self.pos = save;
                return;
            }
            self.initializer2(&mut init.children[i]);
            i += 1;
        }
    }

    fn union_init(&mut self, init: &mut Init) {
        self.expect("{");
        if self.is(".") {
            let i = self.struct_designator(&init.ty.clone());
            init.mem = i;
            self.designation(&mut init.children[i]);
        } else if !self.is("}") && !init.children.is_empty() {
            init.mem = 0;
            self.initializer2(&mut init.children[0]);
        }
        self.consume(",");
        self.expect("}");
    }

    fn initializer2(&mut self, init: &mut Init) {
        let ty = init.ty.clone();
        match &*ty {
            Type::Array(base, _) => {
                let str_at = |p: &Parser, i: usize| p.toks[i].kind == TK::Str && base.is_integer() && base.size() == p.toks[i].ival;
                if str_at(self, self.pos) {
                    self.string_init(init);
                } else if self.is("{") && str_at(self, self.pos + 1) && self.is_at(2, "}") {
                    self.pos += 1;
                    self.string_init(init);
                    self.expect("}");
                } else if self.is("{") {
                    self.array_init1(init);
                } else {
                    self.array_init2(init, 0);
                }
            }
            Type::Struct(sd) => {
                let is_union = sd.borrow().is_union;
                if self.is("{") {
                    if is_union { self.union_init(init) } else { self.struct_init1(init) }
                    return;
                }
                // A struct can be initialized with another struct: `struct T x = y;`
                let save = self.pos;
                let e = self.assign();
                if e.ty.is_struct() {
                    init.expr = Some(e);
                    return;
                }
                self.pos = save;
                if !is_union {
                    self.struct_init2(init, 0, false);
                } else if !init.children.is_empty() {
                    init.mem = 0;
                    self.initializer2(&mut init.children[0]);
                }
            }
            _ => {
                if self.consume("{") {
                    self.initializer2(init);
                    self.consume(",");
                    self.expect("}");
                } else {
                    init.expr = Some(self.assign());
                }
            }
        }
    }

    fn initializer(&mut self, ty: &Ty) -> Init {
        let mut init = new_init(ty, true);
        self.initializer2(&mut init);
        if let (true, Type::Array(base, _)) = (init.flexible, &**ty) {
            init.ty = array_of(base.clone(), init.children.len() as i64);
            init.flexible = false;
        }
        init
    }

    /// Emits assignments initializing the lvalue `target`.
    fn lvar_init(&mut self, init: Init, target: Node, out: &mut Vec<Node>) {
        let loc = target.loc;
        if let Some(e) = init.expr {
            out.push(new_assign(target, e, loc));
            return;
        }
        match &*init.ty {
            Type::Array(..) => {
                for (i, c) in init.children.into_iter().enumerate() {
                    let elem = new_deref(new_add(target.clone(), num(i as i64, loc), loc), loc);
                    self.lvar_init(c, elem, out);
                }
            }
            Type::Struct(sd) => {
                let (is_union, members) = (sd.borrow().is_union, sd.borrow().members.clone());
                for (i, c) in init.children.into_iter().enumerate() {
                    if !is_union || i == init.mem {
                        self.lvar_init(c, new_member(target.clone(), &members[i], loc), out);
                    }
                }
            }
            _ => {}
        }
    }

    fn lvar_initializer(&mut self, l: usize) -> Node {
        let loc = self.loc();
        let init = self.initializer(&self.locals[l].ty.clone());
        self.locals[l].ty = init.ty.clone();
        let mut out = vec![];
        if init.expr.is_none() {
            out.push(mk(NK::MemZero(l), ty_void(), loc));
        }
        let var = self.lvar(l, loc);
        self.lvar_init(init, var, &mut out);
        out.into_iter().reduce(|a, b| new_comma(a, b, loc)).unwrap()
    }

    fn write_gvar_data(&mut self, init: &Init, buf: &mut [u8], off: i64, relocs: &mut Vec<Reloc>) {
        let put = |buf: &mut [u8], off: i64, v: i64, size: i64| {
            buf[off as usize..(off + size) as usize].copy_from_slice(&v.to_le_bytes()[..size as usize]);
        };
        match &*init.ty {
            Type::Array(base, _) => {
                let sz = base.size();
                for (i, c) in init.children.iter().enumerate() {
                    self.write_gvar_data(c, buf, off + sz * i as i64, relocs);
                }
            }
            Type::Struct(sd) => {
                if let Some(e) = &init.expr {
                    err(e.loc, "initializer element is not constant");
                }
                let sd = sd.borrow();
                for (i, (m, c)) in sd.members.iter().zip(&init.children).enumerate() {
                    if sd.is_union && i != init.mem {
                        continue;
                    }
                    if m.bit_width < 0 {
                        self.write_gvar_data(c, buf, off + m.offset, relocs);
                    } else if let Some(e) = &c.expr {
                        let (o, sz) = (off + m.offset, m.ty.size());
                        let mut old = [0u8; 8];
                        old[..sz as usize].copy_from_slice(&buf[o as usize..(o + sz) as usize]);
                        let mask = (1u64 << m.bit_width) - 1;
                        let v = u64::from_le_bytes(old) & !(mask << m.bit_off) | ((eval(e) as u64 & mask) << m.bit_off);
                        put(buf, o, v as i64, sz);
                    }
                }
            }
            ty => {
                let Some(e) = &init.expr else { return };
                match ty {
                    Type::Float => put(buf, off, (eval_double(e) as f32).to_bits() as i64, 4),
                    Type::Double => put(buf, off, eval_double(e).to_bits() as i64, 8),
                    _ => {
                        let mut label = String::new();
                        let v = eval2(e, Some(&mut label));
                        if label.is_empty() {
                            put(buf, off, v, ty.size());
                        } else {
                            relocs.retain(|r| r.offset != off);
                            relocs.push(Reloc { offset: off, label, addend: v });
                        }
                    }
                }
            }
        }
    }

    fn gvar_initializer(&mut self, i: usize) {
        let init = self.initializer(&self.globals[i].ty.clone());
        let mut buf = vec![0u8; init.ty.size() as usize];
        let mut relocs = vec![];
        self.write_gvar_data(&init, &mut buf, 0, &mut relocs);
        relocs.sort_by_key(|r| r.offset);
        let g = &mut self.globals[i];
        g.ty = init.ty;
        g.init = Some(buf);
        g.relocs = relocs;
        g.is_def = true;
    }

    // ----- statements -----

    /// Local declaration: the declarators after the declaration specifiers.
    fn declaration(&mut self, base: Ty, attr: &Attr) -> Node {
        let loc = self.loc();
        let (mut out, mut first) = (vec![], true);
        while !self.consume(";") {
            if !first {
                self.expect(",");
            }
            first = false;
            let (ty, name) = self.declarator(base.clone());
            self.skip_attrs();
            let Some(name) = name else { self.err("variable name omitted") };
            if let Type::Func(_) = &*ty {
                let i = self.declare_func(&name, ty, attr);
                self.scope().vars.insert(name, VS::Global(i));
                continue;
            }
            if attr.is_extern {
                let i = self.get_or_new_global(&name, ty);
                self.scope().vars.insert(name, VS::Global(i));
                continue;
            }
            if attr.is_static {
                let i = self.new_anon_global(&name, ty);
                self.scope().vars.insert(name, VS::Global(i));
                if self.consume("=") {
                    self.gvar_initializer(i);
                }
                continue;
            }
            if let Type::Void = &*ty {
                self.err("variable declared void");
            }
            let l = self.new_lvar(&name, ty);
            if self.consume("=") {
                let e = self.lvar_initializer(l);
                out.push(mk(NK::ExprStmt(bx(e)), ty_void(), loc));
            }
        }
        mk(NK::Block(out), ty_void(), loc)
    }

    /// Parses statements up to the closing brace (the opening one is consumed).
    fn block(&mut self) -> Node {
        let loc = self.loc();
        let mut v = vec![];
        while !self.consume("}") {
            if self.tok().kind == TK::Eof {
                self.err("expected '}'");
            }
            if self.is("_Static_assert") {
                self.skip_to_semi();
            } else if self.is_typename() && !self.is_at(1, ":") {
                let mut attr = Attr::default();
                let base = self.declspec(Some(&mut attr));
                if attr.is_typedef {
                    self.parse_typedef(base);
                } else {
                    v.push(self.declaration(base, &attr));
                }
            } else {
                v.push(self.stmt());
            }
        }
        mk(NK::Block(v), ty_void(), loc)
    }

    /// Statement following a label; a label may also end a block.
    fn labeled(&mut self, label: String, loc: Loc) -> Node {
        let body = if self.is("}") { mk(NK::Nop, ty_void(), loc) } else { self.stmt() };
        mk(NK::Label(label, bx(body)), ty_void(), loc)
    }

    fn loop_body(&mut self, brk: &str, cont: &str) -> Node {
        let old = (self.brk.replace(brk.to_string()), self.cont.replace(cont.to_string()));
        let body = self.stmt();
        (self.brk, self.cont) = old;
        body
    }

    fn stmt(&mut self) -> Node {
        let loc = self.loc();
        let void = ty_void();
        if self.consume("return") {
            if self.consume(";") {
                return mk(NK::Return(None), void, loc);
            }
            let mut e = self.expr();
            self.expect(";");
            if let Type::Void = &*self.ret_ty {
                return mk(NK::Block(vec![mk(NK::ExprStmt(bx(e)), void.clone(), loc), mk(NK::Return(None), void.clone(), loc)]), void, loc);
            }
            if !self.ret_ty.is_struct() {
                e = new_cast(e, self.ret_ty.clone());
            }
            return mk(NK::Return(Some(bx(e))), void, loc);
        }
        if self.consume("if") {
            self.expect("(");
            let c = self.expr();
            self.expect(")");
            let t = self.stmt();
            let e = if self.consume("else") { Some(bx(self.stmt())) } else { None };
            return mk(NK::If(bx(c), bx(t), e), void, loc);
        }
        if self.consume("switch") {
            self.expect("(");
            let c = self.expr();
            let c = new_cast(c.clone(), promote(&c.ty));
            self.expect(")");
            let brk = self.label();
            let old = self.brk.replace(brk.clone());
            self.switches.push(SwitchCtx::default());
            let body = self.stmt();
            let sw = self.switches.pop().unwrap();
            self.brk = old;
            return mk(NK::Switch { cond: bx(c), body: bx(body), cases: sw.cases, default: sw.default, brk }, void, loc);
        }
        if self.consume("case") {
            let v = self.const_expr();
            self.expect(":");
            let l = self.label();
            let Some(sw) = self.switches.last_mut() else { err(loc, "stray case") };
            sw.cases.push((v, l.clone()));
            return self.labeled(l, loc);
        }
        if self.consume("default") {
            self.expect(":");
            let l = self.label();
            let Some(sw) = self.switches.last_mut() else { err(loc, "stray default") };
            sw.default = Some(l.clone());
            return self.labeled(l, loc);
        }
        if self.consume("for") {
            self.expect("(");
            self.scopes.push(Scope::default());
            let init = if self.is_typename() {
                let mut attr = Attr::default();
                let base = self.declspec(Some(&mut attr));
                Some(bx(self.declaration(base, &attr)))
            } else if self.consume(";") {
                None
            } else {
                let e = self.expr();
                self.expect(";");
                Some(bx(mk(NK::ExprStmt(bx(e)), void.clone(), loc)))
            };
            let cond = if self.is(";") { None } else { Some(bx(self.expr())) };
            self.expect(";");
            let inc = if self.is(")") { None } else { Some(bx(self.expr())) };
            self.expect(")");
            let (brk, cont) = (self.label(), self.label());
            let body = bx(self.loop_body(&brk, &cont));
            self.scopes.pop();
            return mk(NK::For { init, cond, inc, body, brk, cont }, void, loc);
        }
        if self.consume("while") {
            self.expect("(");
            let cond = Some(bx(self.expr()));
            self.expect(")");
            let (brk, cont) = (self.label(), self.label());
            let body = bx(self.loop_body(&brk, &cont));
            return mk(NK::For { init: None, cond, inc: None, body, brk, cont }, void, loc);
        }
        if self.consume("do") {
            let (brk, cont) = (self.label(), self.label());
            let body = bx(self.loop_body(&brk, &cont));
            self.expect("while");
            self.expect("(");
            let cond = bx(self.expr());
            self.expect(")");
            self.expect(";");
            return mk(NK::Do { body, cond, brk, cont }, void, loc);
        }
        if self.consume("goto") {
            let name = self.ident();
            self.expect(";");
            return mk(NK::Goto(format!(".L.u.{}.{}", self.fn_name, name)), void, loc);
        }
        if self.consume("break") {
            self.expect(";");
            let Some(l) = self.brk.clone() else { err(loc, "stray break") };
            return mk(NK::Goto(l), void, loc);
        }
        if self.consume("continue") {
            self.expect(";");
            let Some(l) = self.cont.clone() else { err(loc, "stray continue") };
            return mk(NK::Goto(l), void, loc);
        }
        if self.tok().kind == TK::Ident && self.is_at(1, ":") {
            let name = self.ident();
            self.pos += 1;
            return self.labeled(format!(".L.u.{}.{}", self.fn_name, name), loc);
        }
        if self.consume("{") {
            self.scopes.push(Scope::default());
            let b = self.block();
            self.scopes.pop();
            return b;
        }
        if self.consume(";") {
            return mk(NK::Nop, void, loc);
        }
        if self.is("__asm__") || self.is("__asm") {
            self.err("inline assembly is not supported");
        }
        let e = self.expr();
        self.expect(";");
        mk(NK::ExprStmt(bx(e)), void, loc)
    }

    // ----- expressions -----

    fn const_expr(&mut self) -> i64 {
        let n = self.conditional();
        eval(&n)
    }

    fn expr(&mut self) -> Node {
        let n = self.assign();
        let loc = self.loc();
        if self.consume(",") {
            let r = self.expr();
            return new_comma(n, r, loc);
        }
        n
    }

    /// Lowers `lhs op= rhs`.
    fn op_assign(&mut self, op: Op, lhs: Node, rhs: Node, loc: Loc) -> Node {
        if matches!(lhs.kind, NK::LVar(_) | NK::GVar(_)) {
            let b = new_binary(op, lhs.clone(), rhs, loc);
            return new_assign(lhs, b, loc);
        }
        if let NK::Member(base, m) = &lhs.kind {
            if m.bit_width >= 0 {
                // tmp = &base, tmp->m = tmp->m op rhs
                let tmp = self.new_lvar("", ptr_to(base.ty.clone()));
                let set = new_assign(self.lvar(tmp, loc), new_addr((**base).clone(), loc), loc);
                let mem = new_member(new_deref(self.lvar(tmp, loc), loc), m, loc);
                let b = new_binary(op, mem.clone(), rhs, loc);
                return new_comma(set, new_assign(mem, b, loc), loc);
            }
        }
        // tmp = &lhs, *tmp = *tmp op rhs
        let tmp = self.new_lvar("", ptr_to(lhs.ty.clone()));
        let set = new_assign(self.lvar(tmp, loc), new_addr(lhs, loc), loc);
        let val = new_deref(self.lvar(tmp, loc), loc);
        let b = new_binary(op, val.clone(), rhs, loc);
        new_comma(set, new_assign(val, b, loc), loc)
    }

    fn assign(&mut self) -> Node {
        let n = self.conditional();
        let loc = self.loc();
        if self.consume("=") {
            let r = self.assign();
            return new_assign(n, r, loc);
        }
        const OPS: [(&str, Op); 10] = [
            ("+=", Op::Add), ("-=", Op::Sub), ("*=", Op::Mul), ("/=", Op::Div), ("%=", Op::Mod),
            ("&=", Op::BitAnd), ("|=", Op::BitOr), ("^=", Op::BitXor), ("<<=", Op::Shl), (">>=", Op::Shr),
        ];
        for (p, op) in OPS {
            if self.consume(p) {
                let r = self.assign();
                return self.op_assign(op, n, r, loc);
            }
        }
        n
    }

    fn conditional(&mut self) -> Node {
        let c = self.binary(0);
        let loc = self.loc();
        if !self.consume("?") {
            return c;
        }
        let t = self.expr();
        self.expect(":");
        let e = self.conditional();
        let ty = if matches!(*t.ty, Type::Void) || matches!(*e.ty, Type::Void) {
            ty_void()
        } else if t.ty.is_numeric() && e.ty.is_numeric() {
            common_type(&t.ty, &e.ty)
        } else if t.ty.is_integer() {
            decay(&e.ty) // `cond ? 0 : ptr`
        } else {
            decay(&t.ty)
        };
        let (t, e) = if ty.is_numeric() { (new_cast(t, ty.clone()), new_cast(e, ty.clone())) } else { (t, e) };
        mk(NK::Cond(bx(c), bx(t), bx(e)), ty, loc)
    }

    /// Binary operators by precedence level, lowest first.
    fn binary(&mut self, level: usize) -> Node {
        const LEVELS: [&[&str]; 10] = [
            &["||"], &["&&"], &["|"], &["^"], &["&"], &["==", "!="], &["<", "<=", ">", ">="], &["<<", ">>"],
            &["+", "-"], &["*", "/", "%"],
        ];
        if level == LEVELS.len() {
            return self.cast();
        }
        let mut n = self.binary(level + 1);
        loop {
            let loc = self.loc();
            let Some(op) = LEVELS[level].iter().find(|op| self.is(op)) else { return n };
            self.pos += 1;
            let r = self.binary(level + 1);
            n = match *op {
                "||" => mk(NK::LogOr(bx(n), bx(r)), ty_int(), loc),
                "&&" => mk(NK::LogAnd(bx(n), bx(r)), ty_int(), loc),
                "|" => new_binary(Op::BitOr, n, r, loc),
                "^" => new_binary(Op::BitXor, n, r, loc),
                "&" => new_binary(Op::BitAnd, n, r, loc),
                "==" => new_binary(Op::Eq, n, r, loc),
                "!=" => new_binary(Op::Ne, n, r, loc),
                "<" => new_binary(Op::Lt, n, r, loc),
                "<=" => new_binary(Op::Le, n, r, loc),
                ">" => new_binary(Op::Lt, r, n, loc),
                ">=" => new_binary(Op::Le, r, n, loc),
                "<<" => new_binary(Op::Shl, n, r, loc),
                ">>" => new_binary(Op::Shr, n, r, loc),
                "+" => new_binary(Op::Add, n, r, loc),
                "-" => new_binary(Op::Sub, n, r, loc),
                "*" => new_binary(Op::Mul, n, r, loc),
                "/" => new_binary(Op::Div, n, r, loc),
                _ => new_binary(Op::Mod, n, r, loc),
            };
        }
    }

    fn cast(&mut self) -> Node {
        if self.is("(") && self.is_typename_at(self.pos + 1) {
            let (start, loc) = (self.pos, self.loc());
            self.pos += 1;
            let ty = self.typename();
            self.expect(")");
            if self.is("{") {
                // Compound literal
                self.pos = start;
                return self.unary();
            }
            let e = self.cast();
            if ty.is_struct() || matches!(*ty, Type::Void) {
                return mk(NK::Cast(bx(e)), ty, loc);
            }
            if e.ty.is_struct() {
                err(loc, "invalid cast");
            }
            return new_cast(e, ty);
        }
        self.unary()
    }

    fn unary(&mut self) -> Node {
        let loc = self.loc();
        if self.consume("+") {
            let e = self.cast();
            return new_cast(e.clone(), promote(&e.ty));
        }
        if self.consume("-") {
            let e = self.cast();
            let ty = common_type(&ty_int(), &e.ty);
            if let NK::Num(v) = e.kind {
                return mk(NK::Num(wrap(v.wrapping_neg(), &ty)), ty, loc);
            }
            return mk(NK::Neg(bx(new_cast(e, ty.clone()))), ty, loc);
        }
        if self.consume("&") {
            let e = self.cast();
            if matches!(&e.kind, NK::Member(_, m) if m.bit_width >= 0) {
                err(loc, "cannot take address of bitfield");
            }
            return new_addr(e, loc);
        }
        if self.consume("*") {
            let e = self.cast();
            return new_deref(e, loc);
        }
        if self.consume("!") {
            let e = self.cast();
            return mk(NK::Not(bx(e)), ty_int(), loc);
        }
        if self.consume("~") {
            let e = self.cast();
            let ty = promote(&e.ty);
            return mk(NK::BitNot(bx(new_cast(e, ty.clone()))), ty, loc);
        }
        if self.consume("++") {
            let e = self.unary();
            return self.op_assign(Op::Add, e, num(1, loc), loc);
        }
        if self.consume("--") {
            let e = self.unary();
            return self.op_assign(Op::Sub, e, num(1, loc), loc);
        }
        if self.consume("sizeof") {
            let ty = if self.is("(") && self.is_typename_at(self.pos + 1) {
                self.pos += 1;
                let ty = self.typename();
                self.expect(")");
                if self.is("{") { err(loc, "sizeof of a compound literal is not supported") }
                ty
            } else {
                self.unary().ty
            };
            return mk(NK::Num(ty.size()), ty_ulong(), loc);
        }
        if self.consume("_Alignof") || self.consume("__alignof__") {
            self.expect("(");
            let ty = if self.is_typename() { self.typename() } else { self.expr().ty };
            self.expect(")");
            return mk(NK::Num(ty.align()), ty_ulong(), loc);
        }
        self.postfix()
    }

    /// `n++` / `n--` : (typeof n)((n += d) - d)
    fn inc_dec(&mut self, n: Node, d: i64, loc: Loc) -> Node {
        let ty = n.ty.clone();
        let a = self.op_assign(Op::Add, n, num(d, loc), loc);
        new_cast(new_add(a, num(-d, loc), loc), ty)
    }

    fn struct_ref(&mut self, mut n: Node) -> Node {
        let loc = self.loc();
        let name = self.tok().s.clone();
        self.pos += 1;
        let Some(path) = find_member(&n.ty, &name) else { err(loc, &format!("no member named '{}'", name)) };
        for m in &path {
            n = new_member(n, m, loc);
        }
        n
    }

    fn postfix(&mut self) -> Node {
        let loc = self.loc();
        let mut n = if self.is("(") && self.is_typename_at(self.pos + 1) {
            // Compound literal
            self.pos += 1;
            let ty = self.typename();
            self.expect(")");
            if self.scopes.len() == 1 {
                let i = self.new_anon_global(".L.lit", ty);
                self.gvar_initializer(i);
                self.gvar(i, loc)
            } else {
                let l = self.new_lvar("", ty);
                let init = self.lvar_initializer(l);
                new_comma(init, self.lvar(l, loc), loc)
            }
        } else {
            self.primary()
        };
        loop {
            let loc = self.loc();
            if self.consume("(") {
                n = self.funcall(n, loc);
            } else if self.consume("[") {
                let idx = self.expr();
                self.expect("]");
                n = new_deref(new_add(n, idx, loc), loc);
            } else if self.consume(".") {
                n = self.struct_ref(n);
            } else if self.consume("->") {
                n = new_deref(n, loc);
                n = self.struct_ref(n);
            } else if self.consume("++") {
                n = self.inc_dec(n, 1, loc);
            } else if self.consume("--") {
                n = self.inc_dec(n, -1, loc);
            } else {
                return n;
            }
        }
    }

    fn funcall(&mut self, f: Node, loc: Loc) -> Node {
        let fty = match &*f.ty {
            Type::Func(ft) => ft.clone(),
            Type::Ptr(b) => match &**b {
                Type::Func(ft) => ft.clone(),
                _ => err(loc, "called object is not a function"),
            },
            _ => err(loc, "called object is not a function"),
        };
        let mut args = vec![];
        while !self.consume(")") {
            if !args.is_empty() {
                self.expect(",");
            }
            let mut a = self.assign();
            if let Some(pty) = fty.params.get(args.len()) {
                if !pty.is_struct() {
                    a = new_cast(a, pty.clone());
                }
            } else if let Type::Float = &*a.ty {
                a = new_cast(a, ty_double()); // default argument promotion
            } else if a.ty.is_integer() {
                a = new_cast(a.clone(), promote(&a.ty));
            }
            args.push(a);
        }
        let ret_buf = if fty.ret.is_struct() { Some(self.new_lvar("", fty.ret.clone())) } else { None };
        mk(NK::Call { f: bx(f), args, ret_buf }, fty.ret.clone(), loc)
    }

    /// __builtin_va_arg(ap, type), lowered to reads of the va_list structure.
    fn va_arg(&mut self, loc: Loc) -> Node {
        self.expect("(");
        let ap = self.assign();
        self.expect(",");
        let ty = self.typename();
        self.expect(")");

        let tmp = self.new_lvar("", ptr_to(self.va_elem.clone()));
        let set = new_assign(self.lvar(tmp, loc), ap, loc);
        let Type::Struct(sd) = &*self.va_elem else { unreachable!() };
        let members = sd.borrow().members.clone();
        let field = |p: &Parser, i: usize| new_member(new_deref(p.lvar(tmp, loc), loc), &members[i], loc);
        let bump = |p: &Parser, i: usize, by: i64| {
            let f = field(p, i);
            new_assign(f.clone(), new_add(f, num(by, loc), loc), loc)
        };

        let size8 = align_to(ty.size().max(1), 8);
        // Memory class: taken from the overflow area.
        let mem = new_comma(bump(self, 2, size8), new_sub(field(self, 2), num(size8, loc), loc), loc);
        // Register class: (field index, bytes per register, size of the save area)
        let reg = if let Type::Struct(_) = &*ty {
            if ty.size() > 16 {
                None
            } else if !crate::codegen::has_flonum(&ty, 0, 8, 0) && (ty.size() <= 8 || !crate::codegen::has_flonum(&ty, 8, 16, 0)) {
                Some((0, size8, 48))
            } else if ty.size() <= 8 {
                Some((1, 16, 176))
            } else {
                err(loc, "va_arg of a struct passed in SSE registers is not supported")
            }
        } else if ty.is_flonum() {
            Some((1, 16, 176))
        } else {
            Some((0, 8, 48))
        };
        let addr = match reg {
            None => mem,
            Some((i, step, limit)) => {
                let fits = new_binary(Op::Le, field(self, i), num(limit - step, loc), loc);
                let off = new_cast(field(self, i), ty_long());
                let from_reg = new_comma(bump(self, i, step), new_sub(new_add(field(self, 3), off, loc), num(step, loc), loc), loc);
                let pty = from_reg.ty.clone();
                mk(NK::Cond(bx(fits), bx(from_reg), bx(mem)), pty, loc)
            }
        };
        let val = new_deref(mk(NK::Cast(bx(addr)), ptr_to(ty), loc), loc);
        new_comma(set, val, loc)
    }

    fn primary(&mut self) -> Node {
        let loc = self.loc();
        if self.consume("(") {
            if self.consume("{") {
                // GNU statement expression
                self.scopes.push(Scope::default());
                let NK::Block(stmts) = self.block().kind else { unreachable!() };
                self.scopes.pop();
                self.expect(")");
                let ty = match stmts.last() {
                    Some(Node { kind: NK::ExprStmt(e), .. }) => e.ty.clone(),
                    _ => ty_void(),
                };
                return mk(NK::StmtExpr(stmts), ty, loc);
            }
            let n = self.expr();
            self.expect(")");
            return n;
        }
        let t = self.tok().clone();
        match t.kind {
            TK::Num => {
                self.pos += 1;
                match t.nty {
                    NumTy::Float => mk(NK::FNum(t.fval), ty_float(), loc),
                    NumTy::Double => mk(NK::FNum(t.fval), ty_double(), loc),
                    NumTy::Int => mk(NK::Num(t.ival), ty_int(), loc),
                    NumTy::UInt => mk(NK::Num(t.ival), ty_uint(), loc),
                    NumTy::Long => mk(NK::Num(t.ival), ty_long(), loc),
                    NumTy::ULong => mk(NK::Num(t.ival), ty_ulong(), loc),
                }
            }
            TK::Str => {
                self.pos += 1;
                let i = self.new_string(t.bytes, t.ival);
                self.gvar(i, loc)
            }
            TK::Ident => {
                self.pos += 1;
                match t.s.as_str() {
                    "__builtin_va_start" => {
                        self.expect("(");
                        let ap = self.assign();
                        self.expect(",");
                        self.assign();
                        self.expect(")");
                        return mk(NK::VaStart(bx(ap)), ty_void(), loc);
                    }
                    "__builtin_va_end" => {
                        self.expect("(");
                        self.assign();
                        self.expect(")");
                        return mk(NK::Nop, ty_void(), loc);
                    }
                    "__builtin_va_copy" => {
                        self.expect("(");
                        let d = self.assign();
                        self.expect(",");
                        let s = self.assign();
                        self.expect(")");
                        return new_assign(new_deref(d, loc), new_deref(s, loc), loc);
                    }
                    "__builtin_va_arg" => return self.va_arg(loc),
                    "__builtin_expect" => {
                        self.expect("(");
                        let e = self.assign();
                        self.expect(",");
                        self.assign();
                        self.expect(")");
                        return e;
                    }
                    "__builtin_offsetof" => {
                        self.expect("(");
                        let ty = self.typename();
                        self.expect(",");
                        let mut n = self.struct_ref(new_deref(mk(NK::Num(0), ptr_to(ty), loc), loc));
                        loop {
                            if self.consume(".") {
                                n = self.struct_ref(n);
                            } else if self.consume("[") {
                                let idx = self.expr();
                                self.expect("]");
                                n = new_deref(new_add(n, idx, loc), loc);
                            } else {
                                break;
                            }
                        }
                        self.expect(")");
                        return mk(NK::Num(eval(&new_addr(n, loc))), ty_ulong(), loc);
                    }
                    _ => {}
                }
                match self.find_var(&t.s).cloned() {
                    Some(VS::Local(i)) => self.lvar(i, loc),
                    Some(VS::Global(i)) => self.gvar(i, loc),
                    Some(VS::Enum(v)) => num(v, loc),
                    _ if self.is("(") => {
                        // Implicit function declaration (C89)
                        eprintln!("{}:{}: warning: implicit declaration of function '{}'", crate::lex::file_name(loc.0), loc.1, t.s);
                        let fty = Rc::new(Type::Func(Rc::new(FuncTy { ret: ty_int(), params: vec![], names: vec![], variadic: false, oldstyle: true })));
                        let i = self.declare_func(&t.s, fty, &Attr::default());
                        self.scopes[0].vars.insert(t.s.clone(), VS::Global(i));
                        self.gvar(i, loc)
                    }
                    _ if matches!(t.s.as_str(), "__func__" | "__FUNCTION__" | "__PRETTY_FUNCTION__") => {
                        let i = self.new_string(self.fn_name.clone().into_bytes(), 1);
                        self.gvar(i, loc)
                    }
                    _ => err(loc, &format!("undefined variable '{}'", t.s)),
                }
            }
            _ => err(loc, &format!("expected an expression before '{}'", t.s)),
        }
    }
}
