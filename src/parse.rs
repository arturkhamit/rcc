// Recursive-descent parser producing a typed AST. Structure follows chibicc.
use crate::lex::{error_at, NumTy, Token, TK};
use crate::types::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type Loc = (u32, u32); // (file, line)

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    LNum(u64, u16), // long double constant: mantissa, sign and exponent
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
    Undef(usize), // a statement: the local is declared here without an initializer
    Alloca(Box<Node>),
    VlaAlloc { var: usize, size: Box<Node>, save: usize },
    VaStart(Box<Node>),
    StmtExpr(Vec<Node>),
    LabelAddr(String), // GNU `&&label`
    FrameAddr,         // %rbp, for __builtin_frame_address
    // Statements
    Return(Option<Box<Node>>),
    If(Box<Node>, Box<Node>, Option<Box<Node>>),
    For { init: Option<Box<Node>>, cond: Option<Box<Node>>, inc: Option<Box<Node>>, body: Box<Node>, brk: String, cont: String },
    Do { body: Box<Node>, cond: Box<Node>, brk: String, cont: String },
    Switch { cond: Box<Node>, body: Box<Node>, cases: Vec<(i64, i64, String)>, default: Option<String>, brk: String }, // case lo ... hi
    GotoExpr(Box<Node>), // GNU `goto *p`
    Asm(String),         // basic asm statement
    AsmOps(String, Vec<AsmOp>, Vec<String>), // asm with operands: template, operands, clobbers
    Label(String, Box<Node>),
    Goto(String),
    Block(Vec<Node>),
    ExprStmt(Box<Node>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum AsmClass { Reg, Mem, Imm }

/// An operand of an asm statement.
#[derive(Clone)]
pub struct AsmOp {
    pub cons: String,         // the constraint without its `=`, `+`, `&`, `%` modifiers
    pub name: Option<String>, // `[name]`
    pub class: AsmClass,
    pub node: Node, // Reg: a local holding the value; Mem: an lvalue; Imm: a constant
    pub out: bool,  // the template writes it
    pub inp: bool,  // the template reads it
}

pub struct Local {
    pub ty: Ty,
    pub align: i64, // from _Alignas / aligned; 0: natural
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
    pub is_tls: bool,
    pub align: i64, // from _Alignas / aligned; 0: natural
    pub sym: SymAttr,
    pub init: Option<Vec<u8>>,
    pub relocs: Vec<Reloc>,
    pub params: Vec<usize>,
    pub locals: Vec<Local>,
    pub body: Option<Node>,
    pub va_area: Option<usize>,
    pub vla_saves: Vec<usize>, // locals remembering %rsp at VLA declarations
}

/// Attributes of a symbol: `weak`, `alias("target")`, `section("name")`,
/// `constructor`, `destructor`, `noinline`, `visibility("hidden")` and `asm("name")`.
#[derive(Default, Clone)]
pub struct SymAttr {
    pub weak: bool,
    pub hidden: bool,
    pub ctor: bool,
    pub dtor: bool,
    pub noinline: bool,
    pub alias: Option<String>,
    pub section: Option<String>,
    asm_name: Option<String>,
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
    labels: HashMap<String, String>, // GNU `__label__` block-local labels
    taligns: HashMap<String, i64>,   // typedef names declared with `aligned(n)`
}

#[derive(Default)]
struct Attr {
    is_typedef: bool,
    is_static: bool,
    is_extern: bool,
    is_inline: bool,
    is_tls: bool,
    align: i64,
    sym: SymAttr,
}

#[derive(Default)]
struct SwitchCtx {
    cases: Vec<(i64, i64, String)>,
    default: Option<String>,
}

#[derive(Clone)]
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
        Type::Struct(sd) => {
            // Only the last member can be a flexible array.
            let sd = sd.borrow();
            let last = sd.members.len().wrapping_sub(1);
            init.children = sd.members.iter().enumerate().map(|(i, m)| new_init(&m.ty, allow_flex && i == last && !sd.is_union)).collect();
        }
        _ => {}
    }
    init
}

const TYPE_KW: &[&str] = &[
    "void", "_Bool", "char", "short", "int", "long", "struct", "union", "typedef", "enum", "static", "extern",
    "_Alignas", "signed", "unsigned", "const", "volatile", "auto", "register", "restrict", "__restrict",
    "__restrict__", "_Noreturn", "float", "double", "typeof", "__typeof__", "__typeof", "inline", "__inline",
    "__inline__", "_Thread_local", "__thread", "_Atomic", "__attribute__", "__signed__", "__signed", "__const",
    "__volatile__", "__volatile", "__int128",
];

/// Bytes a flexible array member's initializer adds after its struct.
fn flex_extra(init: &Init) -> i64 {
    match (&*init.ty, init.children.last()) {
        (Type::Struct(_), Some(c)) if c.flexible => c.children.len() as i64 * c.ty.base().map_or(0, |b| b.size()),
        _ => 0,
    }
}

fn err(loc: Loc, msg: &str) -> ! {
    error_at(loc.0, loc.1, msg)
}

/// The qualifier a keyword stands for (0 for `restrict`, `register` and the like).
fn qualifier(keyword: &str) -> u8 {
    match keyword {
        "const" | "__const" => Q_CONST,
        "volatile" | "__volatile__" | "__volatile" => Q_VOLATILE,
        "_Atomic" => Q_ATOMIC,
        _ => 0,
    }
}

/// True for an atomic type that fits a register: its operators are made atomic.
/// shortcut: larger atomic objects (structs, long double) are read and written
/// like plain ones; they would need a lock, as in libatomic.
fn atomic_scalar(ty: &Ty) -> bool {
    ty.is_atomic() && ty.size() <= 8 && (ty.is_numeric() || matches!(**ty, Type::Ptr(_)))
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
        (Type::Bool, Type::Bool) | (Type::Float, Type::Float) | (Type::Double, Type::Double) | (Type::LDouble, Type::LDouble) => true,
        _ => false,
    };
    if same {
        return e;
    }
    let loc = e.loc;
    mk(NK::Cast(bx(e)), ty, loc)
}

/// Bitfields narrower than int promote to int, whatever their declared type.
fn bf(n: Node) -> Node {
    match &n.kind {
        NK::Member(_, m) if (0..32).contains(&m.bit_width) && n.ty.is_integer() => {
            let loc = n.loc;
            mk(NK::Cast(bx(n)), ty_int(), loc)
        }
        NK::Member(_, Member { cast: Some(ty), .. }) => {
            let (loc, ty) = (n.loc, ty.clone());
            mk(NK::Cast(bx(n)), ty, loc)
        }
        _ => n,
    }
}

fn arith(op: Op, l: Node, r: Node, loc: Loc) -> Node {
    let (l, r) = (bf(l), bf(r));
    if !l.ty.is_numeric() && l.ty.base().is_none() && !matches!(*l.ty, Type::Func(_)) || r.ty.is_record() {
        err(loc, "invalid operands");
    }
    let ty = common_type(&l.ty, &r.ty);
    let rty = if matches!(op, Op::Eq | Op::Ne | Op::Lt | Op::Le) { ty_int() } else { ty.clone() };
    mk(NK::Bin(op, bx(new_cast(l, ty.clone())), bx(new_cast(r, ty))), rty, loc)
}

/// Size of a type as an expression (a runtime value for VLAs).
fn size_of(ty: &Ty, loc: Loc) -> Node {
    match &**ty {
        Type::Vla(_, sv) => mk(NK::LVar(*sv), ty_ulong(), loc),
        _ => mk(NK::Num(ty.size()), ty_ulong(), loc),
    }
}

/// Scales an index by the size of the pointee type.
fn scale(idx: Node, base: &Ty, loc: Loc) -> Node {
    let idx = new_cast(idx, ty_long());
    let size = new_cast(size_of(base, loc), ty_long());
    if let (NK::Num(v), NK::Num(sz)) = (&idx.kind, &size.kind) {
        return lnum(v.wrapping_mul(*sz), loc);
    }
    mk(NK::Bin(Op::Mul, bx(idx), bx(size)), ty_long(), loc)
}

/// True for integer constant expressions (address constants excluded).
fn is_const(n: &Node) -> bool {
    match &n.kind {
        NK::Num(_) | NK::FNum(_) | NK::LNum(..) => true,
        NK::Bin(_, l, r) | NK::LogAnd(l, r) | NK::LogOr(l, r) => is_const(l) && is_const(r),
        NK::Neg(l) | NK::Not(l) | NK::BitNot(l) | NK::Cast(l) => is_const(l),
        NK::Cond(c, t, e) => is_const(c) && is_const(t) && is_const(e),
        _ => false,
    }
}

/// A function designator in pointer arithmetic is a pointer to the function
/// (GNU: with an element size of 1).
fn fn_decay(n: Node) -> Node {
    if let Type::Func(_) = &*n.ty {
        let (loc, ty) = (n.loc, ptr_to(n.ty.clone()));
        return mk(NK::Cast(bx(n)), ty, loc);
    }
    n
}

fn new_add(l: Node, r: Node, loc: Loc) -> Node {
    let (l, r) = (fn_decay(l), fn_decay(r));
    if l.ty.is_numeric() && r.ty.is_numeric() {
        return arith(Op::Add, l, r, loc);
    }
    let (l, r) = if l.ty.base().is_none() && r.ty.base().is_some() { (r, l) } else { (l, r) };
    let Some(base) = l.ty.base().cloned() else { err(loc, "invalid operands") };
    if !r.ty.is_integer() {
        err(loc, "invalid operands");
    }
    let r = scale(r, &base, loc);
    mk(NK::Bin(Op::Add, bx(l), bx(r)), ptr_to(base), loc)
}

fn new_sub(l: Node, r: Node, loc: Loc) -> Node {
    let (l, r) = (fn_decay(l), fn_decay(r));
    if l.ty.is_numeric() && r.ty.is_numeric() {
        return arith(Op::Sub, l, r, loc);
    }
    let Some(base) = l.ty.base().cloned() else { err(loc, "invalid operands") };
    if r.ty.is_integer() {
        let r = scale(r, &base, loc);
        return mk(NK::Bin(Op::Sub, bx(l), bx(r)), ptr_to(base), loc);
    }
    if r.ty.base().is_none() {
        err(loc, "invalid operands");
    }
    let diff = mk(NK::Bin(Op::Sub, bx(l), bx(r)), ty_long(), loc);
    let size = match &*base {
        Type::Vla(..) => new_cast(size_of(&base, loc), ty_long()),
        _ => lnum(base.size().max(1), loc),
    };
    mk(NK::Bin(Op::Div, bx(diff), bx(size)), ty_long(), loc)
}

fn new_binary(op: Op, l: Node, r: Node, loc: Loc) -> Node {
    match op {
        Op::Add => new_add(l, r, loc),
        Op::Sub => new_sub(l, r, loc),
        Op::Shl | Op::Shr => {
            let l = bf(l);
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
    if let Type::Array(..) | Type::Vla(..) = &*l.ty {
        err(loc, "not an lvalue");
    }
    let r = if l.ty.is_record() { r } else { new_cast(r, l.ty.clone()) };
    let ty = l.ty.clone();
    mk(NK::Assign(bx(l), bx(r)), ty, loc)
}

fn new_comma(l: Node, r: Node, loc: Loc) -> Node {
    let ty = r.ty.clone();
    mk(NK::Comma(bx(l), bx(r)), ty, loc)
}

fn new_member(n: Node, m: &Member, loc: Loc) -> Node {
    // A member of a const or volatile object is const or volatile too.
    let ty = m.ty.qualify(n.ty.quals & (Q_CONST | Q_VOLATILE));
    mk(NK::Member(bx(n), m.clone()), ty, loc)
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
        // An array-typed lvalue evaluates to its address.
        NK::Deref(l) if matches!(&*n.ty, Type::Array(..)) => eval2(l, label),
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
        NK::LabelAddr(l) => {
            match label.as_deref_mut() {
                Some(out) => *out = l.clone(),
                None => err(n.loc, "not a compile-time constant"),
            }
            0
        }
        NK::Num(v) => *v,
        _ => err(n.loc, "not a compile-time constant"),
    };
    wrap(v, &n.ty)
}

/// The value of a constant expression as a 128-bit integer; None if it is not one.
fn eval128(n: &Node) -> Option<i128> {
    let Some(unsigned) = n.ty.i128() else {
        // Of an ordinary integer type: evaluated as such, then widened.
        if !is_const(n) || !n.ty.is_integer() {
            return None;
        }
        let v = eval(n);
        return Some(if n.ty.is_unsigned() && n.ty.size() == 8 { v as u64 as i128 } else { v as i128 });
    };
    Some(match &n.kind {
        NK::Cast(l) => eval128(l)?,
        NK::Neg(l) => eval128(l)?.wrapping_neg(),
        NK::BitNot(l) => !eval128(l)?,
        NK::Cond(c, t, e) => eval128(if eval128(c)? != 0 { t } else { e })?,
        NK::Bin(op, l, r) => {
            let (a, b) = (eval128(l)?, eval128(r)?);
            let (ua, ub) = (a as u128, b as u128);
            match op {
                Op::Add => a.wrapping_add(b),
                Op::Sub => a.wrapping_sub(b),
                Op::Mul => a.wrapping_mul(b),
                Op::Div | Op::Mod if b == 0 => return None,
                Op::Div if unsigned => (ua / ub) as i128,
                Op::Div => a.wrapping_div(b),
                Op::Mod if unsigned => (ua % ub) as i128,
                Op::Mod => a.wrapping_rem(b),
                Op::BitAnd => a & b,
                Op::BitOr => a | b,
                Op::BitXor => a ^ b,
                Op::Shl => a.wrapping_shl(b as u32),
                Op::Shr if unsigned => ua.wrapping_shr(b as u32) as i128,
                Op::Shr => a.wrapping_shr(b as u32),
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// Calls `f` for every node directly below `n`.
fn children_mut(n: &mut Node, f: &mut dyn FnMut(&mut Node)) {
    match &mut n.kind {
        NK::Bin(_, l, r) | NK::LogAnd(l, r) | NK::LogOr(l, r) | NK::Assign(l, r) | NK::Comma(l, r) => {
            f(l);
            f(r);
        }
        NK::Neg(l) | NK::Not(l) | NK::BitNot(l) | NK::Addr(l) | NK::Deref(l) | NK::Cast(l) | NK::Alloca(l) | NK::VaStart(l) | NK::GotoExpr(l) | NK::ExprStmt(l)
        | NK::Member(l, _) | NK::Label(_, l) => f(l),
        NK::VlaAlloc { size, .. } => f(size),
        NK::Cond(c, t, e) => {
            f(c);
            f(t);
            f(e);
        }
        NK::Call { f: callee, args, .. } => {
            f(callee);
            args.iter_mut().for_each(f);
        }
        NK::StmtExpr(v) | NK::Block(v) => v.iter_mut().for_each(f),
        NK::Return(e) => e.iter_mut().for_each(|e| f(e)),
        NK::If(c, t, e) => {
            f(c);
            f(t);
            e.iter_mut().for_each(|e| f(e));
        }
        NK::For { init, cond, inc, body, .. } => {
            [init, cond, inc].into_iter().flatten().for_each(|e| f(e));
            f(body);
        }
        NK::Do { body, cond, .. } | NK::Switch { cond, body, .. } => {
            f(body);
            f(cond);
        }
        NK::AsmOps(_, ops, _) => ops.iter_mut().for_each(|o| f(&mut o.node)),
        NK::Nop | NK::Num(_) | NK::FNum(_) | NK::LNum(..) | NK::LVar(_) | NK::GVar(_) | NK::MemZero(_) | NK::Undef(_) | NK::LabelAddr(_) | NK::FrameAddr | NK::Goto(_)
        | NK::Asm(_) => {}
    }
}

fn eval_bool(n: &Node) -> bool {
    if n.ty.is_flonum() {
        return eval_double(n) != 0.0;
    }
    // The address of an object is a constant that is never null.
    let mut label = String::new();
    let v = eval2(n, Some(&mut label));
    !label.is_empty() || v != 0
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
        NK::LNum(m, se) => f80_to_f64(*m, *se),
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
    fn_name: String,
    ret_ty: Ty,
    brk: Option<String>,
    cont: Option<String>,
    switches: Vec<SwitchCtx>,
    uniq: usize,
    va_elem: Ty,
    i128: [Ty; 2],    // the 128-bit integer types: signed, unsigned
    uses_i128: bool, // some declaration so far has such a type
    vla_pending: Vec<Node>, // size computations of VLA types parsed so far
    vla_saves: Vec<usize>,
    in_params: usize,
    dropped_dims: bool,
    fn_params: (usize, bool), // last parameter list: token position, had variable dimensions
    knr_declared: Option<Vec<Ty>>, // declared parameter types of the K&R definition being parsed
    // Collected by skip_attrs / _Alignas since they were last reset.
    attr_packed: bool,
    attr_align: i64,
    attr_mode: u8, // __attribute__((mode(..))): integer size, 0 if absent
    attr_sym: SymAttr,
    attr_talign: i64, // alignment of the typedef named in the last declspec, 0 if none
    pub top_asm: Vec<String>, // file-scope asm("...")
    pack_points: Vec<(usize, i64)>, // #pragma pack: (token position, value from there on)
}

pub fn parse(toks: Vec<Token>) -> (Vec<Obj>, Vec<String>) {
    let mut p = new_parser(toks);
    p.program();
    (p.globals, p.top_asm)
}

/// Evaluates an integer constant expression (for #if).
pub fn const_expr_tokens(toks: Vec<Token>) -> i64 {
    let mut p = new_parser(toks);
    let v = p.const_expr();
    if p.tok().kind != TK::Eof {
        p.err("extra tokens in constant expression");
    }
    v
}

fn new_parser(mut toks: Vec<Token>) -> Parser {
    // `#pragma pack` arrives as `__pragma_pack ( ... )`: strip it, remembering
    // the packing in force from each position on.
    let mut pack_points = vec![];
    if toks.iter().any(|t| t.s == "__pragma_pack") {
        let (mut out, mut stack, mut cur, mut i) = (Vec::with_capacity(toks.len()), vec![], 0i64, 0);
        while i < toks.len() {
            if toks[i].s != "__pragma_pack" {
                out.push(toks[i].clone());
                i += 1;
                continue;
            }
            let end = (i..toks.len()).find(|j| toks[*j].is(")")).unwrap_or(toks.len() - 1);
            let args = &toks[(i + 2).min(end)..end];
            let n = args.iter().find(|t| t.kind == TK::Num).map(|t| t.ival);
            match args.first().map(|t| t.s.as_str()) {
                Some("push") => {
                    stack.push(cur);
                    cur = n.unwrap_or(cur);
                }
                Some("pop") => cur = stack.pop().unwrap_or(0),
                _ => cur = n.unwrap_or(0),
            }
            pack_points.push((out.len(), cur));
            i = end + 1;
        }
        toks = out;
    }
    // struct __va_elem as defined by the System V x86-64 ABI
    let mem = |name: &str, ty: Ty, offset: i64| Member { name: name.into(), ty, offset, bit_width: -1, bit_off: 0, cast: None };
    let members = vec![
        mem("gp_offset", ty_uint(), 0),
        mem("fp_offset", ty_uint(), 4),
        mem("overflow_arg_area", ptr_to(ty_char()), 8),
        mem("reg_save_area", ptr_to(ty_char()), 16),
    ];
    let va_elem: Ty = Ty::new(Type::Struct(Rc::new(RefCell::new(SDef { is_union: false, members, size: 24, align: 8, complete: true, i128: None }))));
    let mut p = Parser {
        toks,
        pos: 0,
        scopes: vec![Scope::default()],
        globals: vec![],
        gmap: HashMap::new(),
        locals: vec![],
        fn_name: String::new(),
        ret_ty: ty_void(),
        brk: None,
        cont: None,
        switches: vec![],
        uniq: 0,
        i128: [ty_i128(false), ty_i128(true)],
        uses_i128: false,
        va_elem: va_elem.clone(),
        vla_pending: vec![],
        vla_saves: vec![],
        in_params: 0,
        dropped_dims: false,
        fn_params: (0, false),
        knr_declared: None,
        attr_packed: false,
        attr_align: 0,
        attr_mode: 0,
        attr_sym: SymAttr::default(),
        attr_talign: 0,
        top_asm: vec![],
        pack_points,
    };
    p.scopes[0].vars.insert("__builtin_va_list".into(), VS::Typedef(array_of(va_elem, 1)));
    p.scopes[0].vars.insert("__int128_t".into(), VS::Typedef(p.i128[0].clone()));
    p.scopes[0].vars.insert("__uint128_t".into(), VS::Typedef(p.i128[1].clone()));
    p
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
        loop {
            if self.is_asm() {
                // `int x asm("name")`: the assembly name of the symbol.
                self.pos += 1;
                self.expect("(");
                self.attr_sym.asm_name = Some(self.string_arg());
                self.expect(")");
                continue;
            }
            if !self.consume("__attribute__") {
                return;
            }
            self.expect("(");
            self.expect("(");
            // `packed` and `aligned` are recorded for the caller; the rest is skipped.
            while !self.consume(")") {
                if self.consume(",") {
                    continue;
                }
                let name = self.tok().s.trim_matches('_').to_string();
                self.pos += 1;
                match name.as_str() {
                    "packed" => self.attr_packed = true,
                    "weak" => self.attr_sym.weak = true,
                    "constructor" => self.attr_sym.ctor = true,
                    "destructor" => self.attr_sym.dtor = true,
                    "noinline" => self.attr_sym.noinline = true,
                    "alias" | "section" | "visibility" if self.is("(") => {
                        self.pos += 1;
                        let v = self.string_arg();
                        self.expect(")");
                        match name.as_str() {
                            "alias" => self.attr_sym.alias = Some(v),
                            "section" => self.attr_sym.section = Some(v),
                            _ => self.attr_sym.hidden = v == "hidden" || v == "internal",
                        }
                    }
                    "aligned" if self.consume("(") => {
                        self.attr_align = self.attr_align.max(self.const_expr());
                        self.expect(")");
                    }
                    "aligned" => self.attr_align = self.attr_align.max(16),
                    "mode" => {
                        self.expect("(");
                        self.attr_mode = match self.tok().s.trim_matches('_') {
                            "QI" | "byte" => 1,
                            "HI" => 2,
                            "SI" => 4,
                            "DI" | "word" | "pointer" => 8,
                            _ => self.err("unsupported mode attribute"),
                        };
                        self.pos += 1;
                        self.expect(")");
                    }
                    _ if self.is("(") => self.skip_parens(),
                    _ if self.tok().kind == TK::Eof => self.err("unterminated attribute"),
                    _ => {}
                }
            }
            self.expect(")");
        }
    }
    fn string_arg(&mut self) -> String {
        if self.tok().kind != TK::Str {
            self.err("expected a string literal");
        }
        self.pos += 1;
        String::from_utf8_lossy(&self.toks[self.pos - 1].bytes).into_owned()
    }
    /// Applies symbol attributes to a global: those of the declaration specifiers
    /// and those collected after the declarator.
    fn apply_sym(&mut self, i: usize, base: &SymAttr) {
        let d = std::mem::take(&mut self.attr_sym);
        let s = &mut self.globals[i].sym;
        s.weak |= base.weak || d.weak;
        s.hidden |= base.hidden || d.hidden;
        s.ctor |= base.ctor || d.ctor;
        s.dtor |= base.dtor || d.dtor;
        s.noinline |= base.noinline || d.noinline;
        for (dst, src) in [(&mut s.alias, d.alias.or(base.alias.clone())), (&mut s.section, d.section.or(base.section.clone()))] {
            if src.is_some() {
                *dst = src;
            }
        }
        if let Some(name) = d.asm_name.or(base.asm_name.clone()) {
            self.globals[i].name = name; // lookups by the C name still find it
        }
    }
    fn skip_parens(&mut self) {
        self.expect("(");
        let mut depth = 1;
        while depth > 0 {
            if self.tok().kind == TK::Eof {
                self.err("unterminated parenthesis");
            }
            if self.is("(") {
                depth += 1;
            } else if self.is(")") {
                depth -= 1;
            }
            self.pos += 1;
        }
    }
    /// Assembly name of a user label (`__label__` declarations shadow function-wide ones).
    fn user_label(&self, name: &str) -> String {
        match self.scopes.iter().rev().find_map(|s| s.labels.get(name)) {
            Some(l) => l.clone(),
            None => format!(".L.u.{}.{}", self.fn_name, name),
        }
    }
    fn static_assert(&mut self) {
        let loc = self.loc();
        self.pos += 1;
        self.expect("(");
        let ok = self.const_expr() != 0;
        if self.consume(",") {
            self.pos += 1; // the message
        }
        self.expect(")");
        self.expect(";");
        if !ok {
            err(loc, "static assertion failed");
        }
    }
    fn is_asm(&self) -> bool {
        self.is("asm") || self.is("__asm__") || self.is("__asm")
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
        self.locals.push(Local { ty, align: 0 });
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
            is_tls: false,
            align: 0,
            sym: SymAttr::default(),
            init: None,
            relocs: vec![],
            params: vec![],
            locals: vec![],
            body: None,
            va_area: None,
            vla_saves: vec![],
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
        const INT128: u32 = 1 << 19;

        let mut ty = ty_int();
        let mut counter = 0u32;
        let mut quals = 0u8;
        (self.attr_packed, self.attr_align, self.attr_mode) = (false, 0, 0);
        self.attr_sym = SymAttr::default();
        self.attr_talign = 0;
        while self.is_typename() {
            let s = self.tok().s.clone();
            match s.as_str() {
                "typedef" | "static" | "extern" | "inline" | "__inline" | "__inline__" | "_Thread_local" | "__thread" => {
                    let Some(a) = attr.as_deref_mut() else { self.err("storage class specifier is not allowed in this context") };
                    match s.as_str() {
                        "typedef" => a.is_typedef = true,
                        "static" => a.is_static = true,
                        "extern" => a.is_extern = true,
                        "_Thread_local" | "__thread" => a.is_tls = true,
                        _ => a.is_inline = true,
                    }
                    self.pos += 1;
                    continue;
                }
                // `_Atomic(type)` is a type specifier (below); a bare `_Atomic` is a qualifier.
                "const" | "volatile" | "auto" | "register" | "restrict" | "__restrict" | "__restrict__" | "_Noreturn"
                | "__const" | "__volatile__" | "__volatile" | "_Atomic"
                    if s != "_Atomic" || !self.is_at(1, "(") =>
                {
                    quals |= qualifier(&s);
                    self.pos += 1;
                    continue;
                }
                "__attribute__" => {
                    self.skip_attrs();
                    continue;
                }
                "_Alignas" => {
                    self.pos += 1;
                    self.expect("(");
                    let save = (self.attr_packed, self.attr_align);
                    let n = if self.is_typename() { self.typename().align() } else { self.const_expr() };
                    (self.attr_packed, self.attr_align) = (save.0, save.1.max(n));
                    self.expect(")");
                    continue;
                }
                _ => {}
            }
            let td = match self.find_var(&s) {
                Some(VS::Typedef(t)) if self.tok().kind == TK::Ident => Some(t.clone()),
                _ => None,
            };
            if matches!(s.as_str(), "struct" | "union" | "enum" | "typeof" | "__typeof__" | "__typeof" | "_Atomic") || td.is_some() {
                if counter != 0 {
                    break;
                }
                self.pos += 1;
                ty = match s.as_str() {
                    "struct" => self.struct_decl(false),
                    "union" => self.struct_decl(true),
                    "enum" => self.enum_spec(),
                    "typeof" | "__typeof__" | "__typeof" | "_Atomic" => {
                        self.expect("(");
                        let t = if self.is_typename() { self.typename() } else { self.expr().ty };
                        self.expect(")");
                        // `typeof` of an atomic type is the plain type (as in gcc).
                        if s == "_Atomic" { t.qualify(Q_ATOMIC) } else { t.without(Q_ATOMIC) }
                    }
                    _ => {
                        self.attr_talign = self.scopes.iter().rev().find_map(|sc| sc.taligns.get(&s)).copied().unwrap_or(0);
                        td.unwrap()
                    }
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
                "__int128" => counter += INT128,
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
            } else if c == DOUBLE {
                ty_double()
            } else if c == LONG + DOUBLE {
                ty_ldouble()
            } else if b == INT128 {
                self.i128[u as usize].clone()
            } else {
                self.err("invalid type")
            };
        }
        self.uses_i128 |= ty.i128().is_some();
        if let Some(a) = attr {
            a.align = self.attr_align;
            a.sym = std::mem::take(&mut self.attr_sym);
        }
        if self.attr_mode != 0 && ty.is_integer() {
            ty = ty_ints(self.attr_mode, ty.is_unsigned());
            self.attr_mode = 0;
        }
        ty.qualify(quals)
    }

    fn pointers(&mut self, mut ty: Ty) -> Ty {
        loop {
            self.skip_attrs();
            if self.consume("*") {
                ty = ptr_to(ty);
            } else if ["const", "volatile", "restrict", "__restrict", "__restrict__", "__const", "__volatile__", "__volatile", "_Atomic"].iter().any(|k| self.is(k)) {
                ty = ty.qualify(qualifier(&self.tok().s));
                self.pos += 1;
            } else {
                return ty;
            }
        }
    }

    /// Parses a (possibly abstract) declarator. Returns the type and the name.
    fn declarator(&mut self, ty: Ty) -> (Ty, Option<String>) {
        let ty = self.pointers(ty);
        let nested = self.is("(") && {
            let save = self.pos;
            self.pos += 1;
            self.skip_attrs();
            let r = !self.is_typename() && !self.is(")");
            self.pos = save;
            r
        };
        if nested {
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
            if self.is("*") && self.is_at(1, "]") {
                self.pos += 1; // `[*]`: array of unspecified size in a prototype
            }
            let (mut len, mut len_expr) = (-1, None);
            if self.in_params > 0 && self.dim_uses_variables() {
                // A variable dimension in a parameter list cannot be evaluated here: the
                // prototype gets a placeholder type, and a function definition parses
                // its parameter types again with the parameters in scope.
                let mut depth = 0;
                while depth > 0 || !self.is("]") {
                    depth += self.is("[") as i32 - self.is("]") as i32;
                    self.pos += 1;
                }
                self.dropped_dims = true;
            } else if !self.is("]") {
                let e = self.assign();
                if is_const(&e) {
                    len = eval(&e);
                } else if self.scopes.len() == 1 {
                    err(e.loc, "not a compile-time constant");
                } else {
                    len_expr = Some(e);
                }
            }
            self.expect("]");
            let ty = self.type_suffix(ty);
            let e = match len_expr {
                Some(e) => e,
                // A constant number of variable-length rows is a VLA as well.
                None if len >= 0 && matches!(*ty, Type::Vla(..)) => lnum(len, self.loc()),
                None => return array_of(ty, len),
            };
            // VLA: compute its byte size into a hidden local.
            let loc = e.loc;
            let sv = self.new_lvar("", ty_ulong());
            let size = new_binary(Op::Mul, new_cast(e, ty_ulong()), size_of(&ty, loc), loc);
            self.vla_pending.push(new_assign(self.lvar(sv, loc), size, loc));
            return Ty::new(Type::Vla(ty, sv));
        }
        ty
    }

    /// True if the array dimension at the current position mentions a variable.
    fn dim_uses_variables(&self) -> bool {
        self.toks[self.pos..].iter().take_while(|t| !t.is("]")).any(|t| t.kind == TK::Ident && !matches!(self.find_var(&t.s), Some(VS::Enum(_) | VS::Typedef(_))))
    }

    /// Wraps an expression so that pending VLA size computations run first.
    fn with_vla_sizes(&mut self, n: Node) -> Node {
        let loc = n.loc;
        std::mem::take(&mut self.vla_pending).into_iter().rev().fold(n, |acc, p| new_comma(p, acc, loc))
    }

    fn func_params(&mut self, ret: Ty) -> Ty {
        let start = self.pos;
        if self.in_params == 0 {
            self.dropped_dims = false;
        }
        self.in_params += 1;
        let ty = self.func_params2(ret);
        self.in_params -= 1;
        if self.in_params == 0 {
            self.fn_params = (start, self.dropped_dims);
        }
        ty
    }

    fn func_params2(&mut self, ret: Ty) -> Ty {
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
                return Ty::new(Type::Func(Rc::new(FuncTy { ret, params, names, variadic: false, oldstyle: true })));
            }
        }
        Ty::new(Type::Func(Rc::new(FuncTy { ret, params, names, variadic, oldstyle: false })))
    }

    fn typename(&mut self) -> Ty {
        let base = self.declspec(None);
        self.declarator(base).0
    }

    fn struct_decl(&mut self, is_union: bool) -> Ty {
        // The attributes of the struct itself are kept apart from those of the
        // declaration it appears in.
        let outer = (self.attr_packed, self.attr_align);
        (self.attr_packed, self.attr_align) = (false, 0);
        let ty = self.struct_decl2(is_union);
        (self.attr_packed, self.attr_align) = outer;
        ty
    }

    fn struct_decl2(&mut self, is_union: bool) -> Ty {
        self.skip_attrs();
        let (mut packed, mut salign) = (self.attr_packed, self.attr_align);
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
        let pack = self.pack_points.iter().rev().find(|(p, _)| *p <= self.pos).map_or(0, |p| p.1);
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
        let mut extra: Vec<(i64, bool, i64)> = Vec::new(); // per member: explicit alignment, packed, typedef alignment
        while !self.consume("}") {
            if self.is("_Static_assert") {
                self.static_assert();
                continue;
            }
            let mut attr = Attr::default();
            let base = self.declspec(Some(&mut attr));
            let base_packed = self.attr_packed;
            let talign = self.attr_talign;
            if base.is_struct() && self.consume(";") {
                members.push(Member { name: String::new(), ty: base, offset: 0, bit_width: -1, bit_off: 0, cast: None });
                extra.push((attr.align, base_packed, 0));
                continue;
            }
            let mut first = true;
            while !self.consume(";") {
                if !first {
                    self.expect(",");
                }
                first = false;
                (self.attr_packed, self.attr_align) = (false, 0);
                let (mty, name) = if self.is(":") { (base.clone(), None) } else { self.declarator(base.clone()) };
                let mut m = Member { name: name.unwrap_or_default(), ty: mty, offset: 0, bit_width: -1, bit_off: 0, cast: None };
                if let Type::Vla(..) = &*m.ty {
                    self.err("variable-length array in a struct is not supported");
                }
                if self.consume(":") {
                    m.bit_width = self.const_expr();
                }
                self.skip_attrs();
                let scalar = !matches!(*m.ty, Type::Array(..) | Type::Ptr(_) | Type::Func(_));
                members.push(m);
                extra.push((attr.align.max(self.attr_align), base_packed || self.attr_packed, if scalar { talign } else { 0 }));
            }
        }
        (self.attr_packed, self.attr_align) = (false, 0);
        self.skip_attrs();
        packed |= self.attr_packed;
        salign = salign.max(self.attr_align);

        // Layout (bitfields follow the GCC/System V rules).
        let (mut bits, mut align, mut size) = (0i64, 1i64, 0i64);
        for (m, (explicit, mpacked, talign)) in members.iter_mut().zip(&extra) {
            let (msz, natural) = (m.ty.size(), if *talign > 0 { *talign } else { m.ty.align() });
            let mut mal = if packed || *mpacked { 1 } else { natural };
            if pack > 0 {
                mal = mal.min(pack);
            }
            let tight = mal < natural; // packed: bitfields are not aligned to their unit
            mal = mal.max(*explicit);
            if is_union {
                size = size.max(msz);
                align = align.max(mal);
                continue;
            }
            if m.bit_width == 0 {
                bits = align_to(bits, if tight { 8 } else { msz * 8 });
                continue;
            }
            if m.bit_width > 0 && tight {
                m.offset = bits / 8;
                m.bit_off = bits % 8;
                // Access the smallest unit covering the field.
                // shortcut: that unit can still reach up to 7 bytes past the end of the
                // struct, and a field spanning 9 bytes is rejected; both need byte-wise access.
                let Some(unit) = [1u8, 2, 4, 8].into_iter().find(|s| m.bit_off + m.bit_width <= *s as i64 * 8) else {
                    self.err("this packed bit-field layout is not supported")
                };
                if unit as i64 != msz {
                    if m.bit_width >= 32 {
                        m.cast = Some(m.ty.clone());
                    }
                    m.ty = ty_ints(unit, m.ty.is_unsigned());
                }
                bits += m.bit_width;
            } else if m.bit_width > 0 {
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
        align = align.max(salign);
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
        let tag = if self.tok().kind == TK::Ident { Some(self.ident()) } else { None };
        if !self.consume("{") {
            return tag.and_then(|t| self.find_tag(&t)).filter(|t| !t.is_struct()).unwrap_or_else(ty_int);
        }
        let (mut val, mut negative, mut wide) = (0i64, false, false);
        while !self.consume("}") {
            let name = self.ident();
            self.skip_attrs();
            if self.consume("=") {
                val = self.const_expr();
            }
            negative |= val < 0;
            wide |= val != val as i32 as i64 && val != val as u32 as i64;
            self.scope().vars.insert(name, VS::Enum(val));
            val += 1;
            if !self.consume(",") {
                self.expect("}");
                break;
            }
        }
        // Like GCC, an enum without negative enumerators is unsigned.
        // GNU: enumerators that do not fit in 32 bits make it a 64-bit type.
        let ty = match (wide, negative) {
            (true, true) => ty_long(),
            (true, false) => ty_ulong(),
            (false, true) => ty_int(),
            (false, false) => ty_uint(),
        };
        if let Some(tag) = tag {
            self.scope().tags.insert(tag, ty.clone());
        }
        ty
    }

    fn parse_typedef(&mut self, base: Ty) {
        let base_align = self.attr_align;
        let mut first = true;
        while !self.consume(";") {
            if !first {
                self.expect(",");
            }
            first = false;
            (self.attr_align, self.attr_mode) = (0, 0);
            let (mut ty, name) = self.declarator(base.clone());
            self.skip_attrs();
            if self.attr_mode != 0 && ty.is_integer() {
                ty = ty_ints(self.attr_mode, ty.is_unsigned()); // typedef int T __attribute__((mode(DI)))
            }
            let talign = base_align.max(self.attr_align);
            if talign > 0 && !ty.is_struct() {
                // shortcut: rcc's scalar types carry no alignment, so the alignment of
                // such a typedef is honoured only for struct members declared with it.
                if let Some(name) = &name {
                    self.scope().taligns.insert(name.clone(), talign);
                }
            }
            if let (true, Type::Struct(sd)) = (self.attr_align > 0, &*ty) {
                // `typedef struct {..} T __attribute__((aligned(n)))`
                // shortcut: this raises the alignment of the struct type itself.
                let mut sd = sd.borrow_mut();
                sd.align = sd.align.max(self.attr_align); // like GCC, the size stays
            }
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
            if self.is("[") && self.is_at(1, "[") {
                while !(self.is("]") && self.is_at(1, "]")) {
                    self.pos += 1; // [[attribute]]
                }
                self.pos += 2;
                continue;
            }
            if self.is("_Static_assert") {
                self.static_assert();
                continue;
            }
            if self.is_asm() {
                // File-scope assembly is passed through.
                self.pos += 1;
                self.expect("(");
                let text = self.string_arg();
                self.top_asm.push(text);
                self.expect(")");
                self.expect(";");
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
                self.attr_align = 0;
                let (ty, name) = self.declarator(base.clone());
                self.skip_attrs();
                let align = attr.align.max(self.attr_align);
                let Some(name) = name else { self.err("variable name omitted") };
                if let Type::Func(fty) = &*ty {
                    let mut ty = ty.clone();
                    if self.is_typename() {
                        // K&R definition: parameter declarations follow the declarator.
                        // Arguments arrive with the default promotions applied, so the
                        // function type gets the promoted types and the function converts
                        // them to the declared ones on entry.
                        let mut params = fty.params.clone();
                        while !self.is("{") {
                            let base = self.declspec(None);
                            loop {
                                let (pty, pname) = self.declarator(base.clone());
                                match fty.names.iter().position(|n| Some(n) == pname.as_ref()) {
                                    Some(i) => params[i] = decay(&pty),
                                    None => self.err("declaration for a non-parameter"),
                                }
                                if !self.consume(",") {
                                    break;
                                }
                            }
                            self.expect(";");
                        }
                        let promoted = params.iter().map(|t| if let Type::Float = **t { ty_double() } else { promote(t) }).collect();
                        self.knr_declared = Some(params);
                        ty = Ty::new(Type::Func(Rc::new(FuncTy { ret: fty.ret.clone(), params: promoted, names: fty.names.clone(), variadic: false, oldstyle: false })));
                    }
                    let i = self.declare_func(&name, ty.clone(), &attr);
                    self.globals[i].align = self.globals[i].align.max(align);
                    self.apply_sym(i, &attr.sym);
                    if self.is("{") {
                        self.function_def(ty, &name, &attr);
                        break;
                    }
                    self.scopes[0].vars.insert(name, VS::Global(i));
                    continue;
                }
                let i = self.get_or_new_global(&name, ty);
                self.scopes[0].vars.insert(name, VS::Global(i));
                self.globals[i].is_static |= attr.is_static;
                self.globals[i].is_tls |= attr.is_tls;
                self.globals[i].align = self.globals[i].align.max(align);
                self.apply_sym(i, &attr.sym);
                if !attr.is_extern && self.globals[i].sym.alias.is_none() {
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
        self.vla_pending.clear();
        self.vla_saves.clear();
        self.fn_name = name.to_string();
        self.ret_ty = fty.ret.clone();
        self.scopes.push(Scope::default());
        let mut params = vec![];
        if mem_class(&fty.ret) {
            // Hidden pointer to the caller's return buffer.
            params.push(self.new_lvar("", ptr_to(fty.ret.clone())));
        }
        let mut entry = vec![];
        let knr = self.knr_declared.take();
        for (k, (pty, pname)) in fty.params.iter().zip(&fty.names).enumerate() {
            match knr.as_ref().and_then(|d| d.get(k)) {
                Some(declared) if !same_type(declared, pty) => {
                    // K&R: the promoted argument is converted to the declared type.
                    let (arg, loc) = (self.new_lvar("", pty.clone()), self.loc());
                    params.push(arg);
                    let l = self.new_lvar(pname, declared.clone());
                    let set = new_assign(self.lvar(l, loc), self.lvar(arg, loc), loc);
                    entry.push(mk(NK::ExprStmt(bx(set)), ty_void(), loc));
                }
                _ => params.push(self.new_lvar(pname, pty.clone())),
            }
        }
        let va_area = if fty.variadic { Some(self.new_lvar("", array_of(ty_char(), 176))) } else { None };
        // Variably modified parameter types (`int a[n][m]`): parse the parameter
        // declarations again, now that the parameters are in scope. The size
        // expressions are evaluated on entry to the function.
        if let (start, true) = self.fn_params {
            let (save, loc) = (self.pos, self.loc());
            self.pos = start;
            for k in 0..fty.params.len() {
                if k > 0 {
                    self.expect(",");
                }
                let base = self.declspec(None);
                let ty = self.declarator(base).0;
                self.skip_attrs();
                let l = params[params.len() - fty.params.len() + k];
                self.locals[l].ty = decay(&ty);
            }
            self.pos = save;
            entry.extend(std::mem::take(&mut self.vla_pending).into_iter().map(|e| mk(NK::ExprStmt(bx(e)), ty_void(), loc)));
        }
        let loc = self.loc();
        self.expect("{");
        entry.push(self.block());
        let mut body = mk(NK::Block(entry), ty_void(), loc);
        self.scopes.pop();
        if self.uses_i128 {
            self.lower128(&mut body);
        }

        let g = &mut self.globals[i];
        g.params = params;
        g.va_area = va_area;
        g.vla_saves = std::mem::take(&mut self.vla_saves);
        g.locals = std::mem::take(&mut self.locals);
        g.body = Some(body);
    }

    // ----- 128-bit integers -----

    /// A call of one of the functions of builtins.h (or libgcc) that implement
    /// the 128-bit operations.
    fn call128(&mut self, name: &str, args: Vec<Node>, ret: &Ty, loc: Loc) -> Node {
        let Some(VS::Global(i)) = self.scopes[0].vars.get(name).cloned() else { err(loc, "128-bit integers need the built-in declarations") };
        let ret_buf = if ret.is_struct() { Some(self.new_lvar("", ret.clone())) } else { None };
        let f = self.gvar(i, loc);
        mk(NK::Call { f: bx(f), args, ret_buf }, ret.clone(), loc)
    }

    /// The low word of a 128-bit integer.
    fn low128(n: Node) -> Node {
        let Type::Struct(sd) = &*n.ty else { unreachable!() };
        let (word, loc) = (sd.borrow().members[0].clone(), n.loc);
        new_member(n, &word, loc)
    }

    /// A 128-bit integer used as a condition: whether it is nonzero.
    fn truth128(&mut self, c: &mut Node) {
        if c.ty.i128().is_some() {
            let (loc, v) = (c.loc, std::mem::replace(c, num(0, c.loc)));
            *c = self.call128("__rcc_nz128", vec![v], &ty_int(), loc);
        }
    }

    /// Rewrites the operations on 128-bit integers in a function body as calls
    /// and accesses to their two words. Run when the body is complete; until
    /// then such an integer is an integer like any other to the parser.
    fn lower128(&mut self, n: &mut Node) {
        children_mut(n, &mut |c| self.lower128(c));
        let (loc, wide) = (n.loc, |t: &Ty| t.i128().is_some());
        match &mut n.kind {
            NK::If(c, ..) | NK::Cond(c, ..) | NK::Not(c) | NK::For { cond: Some(c), .. } | NK::Do { cond: c, .. } => self.truth128(c),
            NK::LogAnd(l, r) | NK::LogOr(l, r) => {
                self.truth128(l);
                self.truth128(r);
            }
            NK::Switch { cond, cases, .. } if wide(&cond.ty) => {
                // The case labels are 64-bit constants: a value that is not the
                // extension of its low word matches none of them. The switch is
                // on the low word then, or else on a number that is no label.
                let Type::Struct(sd) = &*cond.ty else { unreachable!() };
                let words = sd.borrow().members.clone();
                let var = self.new_lvar("", cond.ty.clone());
                let var = self.lvar(var, loc);
                let word = |i: usize| new_cast(new_member(var.clone(), &words[i], loc), ty_long());
                let fits = new_binary(Op::Eq, word(1), new_binary(Op::Shr, word(0), num(63, loc), loc), loc);
                let no_label = (i64::MIN..).find(|v| !cases.iter().any(|c| c.0 <= *v && *v <= c.1)).unwrap();
                let low = mk(NK::Cond(bx(fits), bx(word(0)), bx(lnum(no_label, loc))), ty_long(), loc);
                **cond = new_comma(new_assign(var, std::mem::replace(cond, num(0, loc)), loc), low, loc);
            }
            _ => {}
        }
        let ty = n.ty.clone();
        let take = |n: &mut Node| std::mem::replace(n, num(0, loc));
        let new = match &mut n.kind {
            NK::Cast(l) if wide(&ty) && wide(&l.ty) => Node { ty, ..take(l) }, // only the signedness can differ
            NK::Cast(l) if wide(&ty) => {
                let (from, l) = (l.ty.clone(), take(l));
                match (&*from, ty.i128() == Some(true)) {
                    (Type::Float, false) => self.call128("__fixsfti", vec![l], &ty, loc),
                    (Type::Float, true) => self.call128("__fixunssfti", vec![l], &ty, loc),
                    (Type::Double, false) => self.call128("__fixdfti", vec![l], &ty, loc),
                    (Type::Double, true) => self.call128("__fixunsdfti", vec![l], &ty, loc),
                    (Type::LDouble, false) => self.call128("__fixxfti", vec![l], &ty, loc),
                    (Type::LDouble, true) => self.call128("__fixunsxfti", vec![l], &ty, loc),
                    // An integer or a pointer is widened as its own type says.
                    _ if from.is_unsigned() => self.call128("__rcc_zext128", vec![new_cast(l, ty_ulong())], &ty, loc),
                    _ => self.call128("__rcc_sext128", vec![new_cast(l, ty_long())], &ty, loc),
                }
            }
            NK::Cast(l) if wide(&l.ty) && !matches!(*ty, Type::Void) => {
                let (unsigned, l) = (l.ty.i128() == Some(true), take(l));
                match (&*ty, unsigned) {
                    (Type::Bool, _) => new_cast(self.call128("__rcc_nz128", vec![l], &ty_int(), loc), ty),
                    (Type::Float, false) => self.call128("__floattisf", vec![l], &ty, loc),
                    (Type::Float, true) => self.call128("__floatuntisf", vec![l], &ty, loc),
                    (Type::Double, false) => self.call128("__floattidf", vec![l], &ty, loc),
                    (Type::Double, true) => self.call128("__floatuntidf", vec![l], &ty, loc),
                    (Type::LDouble, false) => self.call128("__floattixf", vec![l], &ty, loc),
                    (Type::LDouble, true) => self.call128("__floatuntixf", vec![l], &ty, loc),
                    _ => new_cast(Self::low128(l), ty), // a narrower integer, or a pointer: the low word
                }
            }
            NK::Neg(l) if wide(&ty) => self.call128("__rcc_neg128", vec![take(l)], &ty, loc),
            NK::BitNot(l) if wide(&ty) => self.call128("__rcc_not128", vec![take(l)], &ty, loc),
            NK::Bin(op, l, r) if wide(&l.ty) => {
                let (op, unsigned, wide_ty) = (*op, l.ty.i128() == Some(true), l.ty.clone());
                let (l, r) = (take(l), take(r));
                let not = |n: Node| mk(NK::Not(bx(n)), ty_int(), loc);
                let less = if unsigned { "__rcc_ult128" } else { "__rcc_slt128" };
                match op {
                    Op::Add => self.call128("__rcc_add128", vec![l, r], &ty, loc),
                    Op::Sub => self.call128("__rcc_sub128", vec![l, r], &ty, loc),
                    Op::BitAnd => self.call128("__rcc_and128", vec![l, r], &ty, loc),
                    Op::BitOr => self.call128("__rcc_or128", vec![l, r], &ty, loc),
                    Op::BitXor => self.call128("__rcc_xor128", vec![l, r], &ty, loc),
                    Op::Mul => self.call128("__multi3", vec![l, r], &ty, loc),
                    Op::Div => self.call128(if unsigned { "__udivti3" } else { "__divti3" }, vec![l, r], &ty, loc),
                    Op::Mod => self.call128(if unsigned { "__umodti3" } else { "__modti3" }, vec![l, r], &ty, loc),
                    Op::Shl | Op::Shr => {
                        let count = new_cast(if wide(&r.ty) { Self::low128(r) } else { r }, ty_int());
                        let name = if op == Op::Shl { "__ashlti3" } else if unsigned { "__lshrti3" } else { "__ashrti3" };
                        self.call128(name, vec![l, count], &wide_ty, loc)
                    }
                    Op::Eq => self.call128("__rcc_eq128", vec![l, r], &ty_int(), loc),
                    Op::Ne => not(self.call128("__rcc_eq128", vec![l, r], &ty_int(), loc)),
                    Op::Lt => self.call128(less, vec![l, r], &ty_int(), loc),
                    Op::Le => not(self.call128(less, vec![r, l], &ty_int(), loc)),
                }
            }
            _ => return,
        };
        *n = new;
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

    /// `[i] = v` or GNU `[lo ... hi] = v`. Returns the index after the designated ones.
    fn array_designator(&mut self, init: &mut Init) -> usize {
        self.pos += 1;
        let lo = self.const_expr() as usize;
        let hi = if self.consume("...") { self.const_expr() as usize } else { lo };
        self.expect("]");
        if hi < lo {
            self.err("empty index range in initializer");
        }
        self.ensure_child(init, hi);
        self.designation(&mut init.children[lo]);
        for k in lo + 1..=hi {
            // shortcut: the value expression is duplicated, so its side effects repeat.
            init.children[k] = init.children[lo].clone();
        }
        hi + 1
    }

    fn designation(&mut self, init: &mut Init) {
        if self.is("[") {
            if !matches!(&*init.ty, Type::Array(..)) {
                self.err("array index in non-array initializer");
            }
            let next = self.array_designator(init);
            self.array_init2(init, next);
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
                i = self.array_designator(init);
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
        let mut first = true;
        while !self.consume_end() {
            if !first {
                self.expect(",");
            }
            if self.is(".") {
                // The union holds the member designated last.
                let i = self.struct_designator(&init.ty.clone());
                init.mem = i;
                self.designation(&mut init.children[i]);
            } else if first && !init.children.is_empty() {
                init.mem = 0;
                self.initializer2(&mut init.children[0]);
            } else {
                self.skip_excess();
            }
            first = false;
        }
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
            Type::Struct(sd) if ty.is_record() => {
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
                    if self.consume("}") {
                        return; // `{}`: zero
                    }
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
        if flex_extra(&init) > 0 {
            err(loc, "initialization of a flexible array member of a local variable");
        }
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
            Type::Struct(_) if init.ty.i128().is_some() => {
                let Some(e) = &init.expr else { return };
                let Some(v) = eval128(e) else { err(e.loc, "initializer element is not constant") };
                buf[off as usize..off as usize + 16].copy_from_slice(&v.to_le_bytes());
            }
            Type::Struct(sd) => {
                if let Some(e) = &init.expr {
                    // GNU: a compound literal is a constant here; its bytes are copied.
                    let lit = match &e.kind {
                        NK::GVar(name) if name.starts_with(".L.lit") => self.globals.iter().rev().find(|g| g.name == *name),
                        _ => None,
                    };
                    let Some((data, g)) = lit.and_then(|g| Some((g.init.as_ref()?, g))) else { err(e.loc, "initializer element is not constant") };
                    buf[off as usize..off as usize + data.len()].copy_from_slice(data);
                    relocs.extend(g.relocs.iter().map(|r| Reloc { offset: r.offset + off, label: r.label.clone(), addend: r.addend }));
                    return;
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
                    Type::LDouble => {
                        // shortcut: only a plain constant keeps its 64-bit mantissa; other
                        // constant expressions are folded in double precision.
                        let (m, se) = match &e.kind {
                            NK::LNum(m, se) => (*m, *se),
                            _ => f64_to_f80(eval_double(e)),
                        };
                        put(buf, off, m as i64, 8);
                        put(buf, off + 8, se as i64, 2);
                    }
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
        // The data of a flexible array member follows the struct; the type keeps its size.
        let mut buf = vec![0u8; (init.ty.size() + flex_extra(&init)) as usize];
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
            self.attr_align = 0;
            let (ty, name) = self.declarator(base.clone());
            self.skip_attrs();
            let align = attr.align.max(self.attr_align);
            let Some(name) = name else { self.err("variable name omitted") };
            for e in std::mem::take(&mut self.vla_pending) {
                out.push(mk(NK::ExprStmt(bx(e)), ty_void(), loc));
            }
            if let Type::Func(_) = &*ty {
                let i = self.declare_func(&name, ty, attr);
                self.scope().vars.insert(name, VS::Global(i));
                continue;
            }
            if attr.is_extern {
                let i = self.get_or_new_global(&name, ty);
                self.globals[i].is_tls |= attr.is_tls;
                self.scope().vars.insert(name, VS::Global(i));
                continue;
            }
            if attr.is_static {
                let i = self.new_anon_global(&name, ty);
                self.globals[i].is_tls = attr.is_tls;
                self.globals[i].align = align;
                self.scope().vars.insert(name, VS::Global(i));
                if self.consume("=") {
                    self.gvar_initializer(i);
                }
                continue;
            }
            if let Type::Void = &*ty {
                self.err("variable declared void");
            }
            let l = self.new_lvar(&name, ty.clone());
            self.locals[l].align = align;
            if let Type::Vla(_, sv) = &*ty {
                let save = self.new_lvar("", ty_ulong());
                self.vla_saves.push(save);
                let alloc = NK::VlaAlloc { var: l, size: bx(self.lvar(*sv, loc)), save };
                out.push(mk(NK::ExprStmt(bx(mk(alloc, ty_void(), loc))), ty_void(), loc));
                continue;
            }
            if self.consume("=") {
                let e = self.lvar_initializer(l);
                out.push(mk(NK::ExprStmt(bx(e)), ty_void(), loc));
            } else {
                out.push(mk(NK::Undef(l), ty_void(), loc));
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
                self.static_assert();
            } else if self.consume("__label__") {
                loop {
                    let name = self.ident();
                    let l = format!(".L.u.{}.{}.{}", self.fn_name, name, self.uniq);
                    self.uniq += 1;
                    self.scope().labels.insert(name, l);
                    if !self.consume(",") {
                        break;
                    }
                }
                self.expect(";");
            } else if self.is_typename() && !self.is_at(1, ":") {
                let mut attr = Attr::default();
                let base = self.declspec(Some(&mut attr));
                if attr.is_typedef {
                    self.parse_typedef(base);
                    for e in std::mem::take(&mut self.vla_pending) {
                        v.push(mk(NK::ExprStmt(bx(e)), ty_void(), loc));
                    }
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
            if !self.ret_ty.is_record() {
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
            let hi = if self.consume("...") { self.const_expr() } else { v };
            self.expect(":");
            let l = self.label();
            let Some(sw) = self.switches.last_mut() else { err(loc, "stray case") };
            sw.cases.push((v, hi, l.clone()));
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
            if self.consume("*") {
                let e = self.expr();
                self.expect(";");
                return mk(NK::GotoExpr(bx(e)), void, loc);
            }
            let name = self.ident();
            self.expect(";");
            return mk(NK::Goto(self.user_label(&name)), void, loc);
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
            self.skip_attrs();
            return self.labeled(self.user_label(&name), loc);
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
        if self.is_asm() {
            self.pos += 1;
            while self.consume("volatile") || self.consume("inline") || self.consume("__volatile__") {}
            self.expect("(");
            let text = self.string_arg();
            if self.consume(")") {
                // Basic asm (no operands) is emitted as written.
                self.expect(";");
                return mk(NK::Asm(text), void, loc);
            }
            // Sections after the template: outputs, inputs, clobbers. A register
            // operand goes through a local: assigned before the statement if the
            // template reads it, copied to its lvalue afterwards if the template
            // writes it. The code generator then has nothing to evaluate.
            let (mut section, mut ops, mut clobbers, mut pre, mut post) = (0, vec![], vec![], vec![], vec![]);
            while !self.consume(")") {
                if self.consume(":") {
                    section += 1;
                    continue;
                }
                if self.consume(",") {
                    continue;
                }
                if section == 3 {
                    clobbers.push(self.string_arg());
                    continue;
                }
                if section > 3 {
                    self.err("asm goto is not supported");
                }
                let name = if self.consume("[") { Some(self.ident()) } else { None };
                if name.is_some() {
                    self.expect("]");
                }
                let cons = self.string_arg();
                self.expect("(");
                let e = self.expr();
                self.expect(")");
                let (out, inp) = (section == 1, section == 2 || cons.contains('+'));
                let has = |set: &str| cons.chars().any(|c| set.contains(c));
                let ty = decay(&e.ty);
                let scalar = ty.size() <= 8 && (ty.is_numeric() || matches!(*ty, Type::Ptr(_)));
                let (class, node) = if has("@") {
                    self.err("asm flag outputs (\"=@cc...\") are not supported")
                } else if has("0123456789") || (scalar && has("rqQRlgXabcdSDA")) {
                    let tmp = self.new_lvar("", if out { e.ty.without(!0) } else { ty.without(!0) });
                    let var = self.lvar(tmp, loc);
                    if out {
                        // The lvalue is evaluated once, before the statement (a
                        // bitfield has no address: it is evaluated again to store).
                        let lv = if matches!(&e.kind, NK::LVar(_) | NK::GVar(_)) || matches!(&e.kind, NK::Member(_, m) if m.bit_width >= 0) {
                            e
                        } else {
                            let p = self.new_lvar("", ptr_to(e.ty.clone()));
                            pre.push(new_assign(self.lvar(p, loc), new_addr(e, loc), loc));
                            new_deref(self.lvar(p, loc), loc)
                        };
                        if inp {
                            pre.push(new_assign(var.clone(), lv.clone(), loc));
                        }
                        post.push(new_assign(lv, var.clone(), loc));
                    } else {
                        pre.push(new_assign(var.clone(), e, loc));
                    }
                    (AsmClass::Reg, var)
                } else if has("moVgX") {
                    (AsmClass::Mem, e)
                } else if has("inesIJKLMNO") && is_const(&e) && e.ty.is_integer() {
                    (AsmClass::Imm, lnum(eval(&e), loc))
                } else {
                    // shortcut: no SSE ("x"), x87 ("t", "u") or symbolic immediate operands; add them when a program needs one.
                    self.err(&format!("asm operand constraint \"{}\" is not supported for this operand", cons))
                };
                let cons = cons.chars().filter(|c| !"=+&%".contains(*c)).collect();
                ops.push(AsmOp { cons, name, class, node, out, inp });
            }
            self.expect(";");
            let stmt = |e: Node| mk(NK::ExprStmt(bx(e)), ty_void(), loc);
            let mut out: Vec<Node> = pre.into_iter().map(stmt).collect();
            out.push(mk(NK::AsmOps(text, ops, clobbers), void.clone(), loc));
            out.extend(post.into_iter().map(stmt));
            return mk(NK::Block(out), void, loc);
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
            let ty = decay(&r.ty); // the result is not an lvalue: arrays decay
            return mk(NK::Comma(bx(n), bx(r)), ty, loc);
        }
        n
    }

    /// Assignment to an atomic object as one indivisible step: `lhs = rhs`
    /// (no `op`) is an exchange, `lhs op= rhs` a compare-and-swap loop. Both are
    /// asm statements built here, so the code generator has no atomics of its own.
    fn atomic_assign(&mut self, op: Option<Op>, lhs: Node, rhs: Node, loc: Loc) -> Node {
        let ty = lhs.ty.without(!0);
        let var = |p: &mut Parser, ty: Ty| {
            let i = p.new_lvar("", ty);
            p.lvar(i, loc)
        };
        let stmt = |e: Node| mk(NK::ExprStmt(bx(e)), ty_void(), loc);
        let reg = |cons: &str, node: &Node, out: bool, inp: bool| AsmOp { cons: cons.into(), name: None, class: AsmClass::Reg, node: node.clone(), out, inp };
        let asm = |text: &str, ops: Vec<AsmOp>| mk(NK::AsmOps(text.into(), ops, vec![]), ty_void(), loc);
        let addr = var(self, ptr_to(ty.clone()));
        let val = var(self, if op.is_some() { decay(&rhs.ty) } else { ty.clone() });
        let new = var(self, ty.clone());
        let target = AsmOp { cons: "m".into(), name: None, class: AsmClass::Mem, node: new_deref(addr.clone(), loc), out: true, inp: true };
        let mut stmts = vec![stmt(new_assign(addr.clone(), new_addr(lhs, loc), loc)), stmt(new_assign(val.clone(), rhs, loc))];
        match op {
            None => {
                // new = val; exchange new with *addr. The value of the expression is val.
                stmts.push(stmt(new_assign(new.clone(), val.clone(), loc)));
                stmts.push(asm("xchg %0, %1", vec![reg("r", &new, true, true), target]));
                stmts.push(stmt(val));
            }
            Some(op) => {
                // old = *addr; do new = old op val; while (*addr != old: then old = *addr; else *addr = new);
                let (old, ok) = (var(self, ty.clone()), var(self, ty_bool()));
                stmts.push(stmt(new_assign(old.clone(), new_deref(addr.clone(), loc), loc)));
                let step = stmt(new_assign(new.clone(), new_binary(op, old.clone(), val, loc), loc));
                let cas = asm("lock cmpxchg %3, %1\n\tsete %0", vec![reg("q", &ok, true, false), target, reg("a", &old, true, true), reg("r", &new, false, true)]);
                let body = bx(mk(NK::Block(vec![step, cas]), ty_void(), loc));
                let cond = bx(mk(NK::Not(bx(ok)), ty_int(), loc));
                stmts.push(mk(NK::Do { body, cond, brk: self.label(), cont: self.label() }, ty_void(), loc));
                stmts.push(stmt(new));
            }
        }
        mk(NK::StmtExpr(stmts), ty, loc)
    }

    /// Lowers `lhs op= rhs`.
    fn op_assign(&mut self, op: Op, lhs: Node, rhs: Node, loc: Loc) -> Node {
        if atomic_scalar(&lhs.ty) {
            return self.atomic_assign(Some(op), lhs, rhs, loc);
        }
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
            return if atomic_scalar(&n.ty) { self.atomic_assign(None, n, r, loc) } else { new_assign(n, r, loc) };
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
        // GNU `a ?: b` is `a ? a : b` with `a` evaluated once.
        let (c, t) = if !self.is(":") {
            (c, self.expr())
        } else if is_const(&c) || self.scopes.len() == 1 {
            (c.clone(), c)
        } else {
            let tmp = self.new_lvar("", decay(&c.ty));
            (new_assign(self.lvar(tmp, loc), c, loc), self.lvar(tmp, loc))
        };
        self.expect(":");
        let e = self.conditional();
        let ty = if matches!(*t.ty, Type::Void) || matches!(*e.ty, Type::Void) {
            ty_void()
        } else if t.ty.is_numeric() && e.ty.is_numeric() {
            common_type(&t.ty, &e.ty)
        } else {
            // A null pointer constant takes the other operand's pointer type;
            // otherwise `void *` wins over other pointer types.
            let is_null = |n: &Node| match &n.kind {
                NK::Num(0) => true,
                NK::Cast(l) => matches!(l.kind, NK::Num(0)) && matches!(&*n.ty, Type::Ptr(b) if matches!(**b, Type::Void)),
                _ => false,
            };
            let is_void_ptr = |ty: &Ty| matches!(&**ty, Type::Ptr(b) if matches!(**b, Type::Void));
            let (tp, ep) = (decay(&t.ty), decay(&e.ty));
            if t.ty.is_integer() || (is_null(&t) && ep.base().is_some()) {
                ep
            } else if is_void_ptr(&ep) && tp.base().is_some() && !is_null(&e) {
                ep
            } else {
                tp
            }
        };
        let (t, e) = if ty.is_numeric() {
            (new_cast(t, ty.clone()), new_cast(e, ty.clone()))
        } else if let Type::Void = &*ty {
            (mk(NK::Cast(bx(t)), ty.clone(), loc), mk(NK::Cast(bx(e)), ty.clone(), loc))
        } else {
            (t, e)
        };
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
            if ty.is_record() || matches!(*ty, Type::Void) {
                return mk(NK::Cast(bx(e)), ty, loc);
            }
            if e.ty.is_record() {
                err(loc, "invalid cast");
            }
            if matches!(&e.kind, NK::Member(_, m) if m.bit_width >= 0) {
                return mk(NK::Cast(bx(e)), ty, loc);
            }
            return self.with_vla_sizes(new_cast(e, ty));
        }
        self.unary()
    }

    fn unary(&mut self) -> Node {
        let loc = self.loc();
        if self.consume("+") {
            let e = bf(self.cast());
            return new_cast(e.clone(), promote(&e.ty));
        }
        if self.consume("-") {
            let e = bf(self.cast());
            let ty = common_type(&ty_int(), &e.ty);
            if let NK::Num(v) = e.kind {
                return mk(NK::Num(wrap(v.wrapping_neg(), &ty)), ty, loc);
            }
            if let NK::LNum(m, se) = e.kind {
                return mk(NK::LNum(m, se ^ 0x8000), ty, loc);
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
        if self.consume("&&") {
            let name = self.ident();
            return mk(NK::LabelAddr(self.user_label(&name)), ptr_to(ty_void()), loc);
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
            let e = bf(self.cast());
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
            let start = self.pos;
            let ty = if self.is("(") && self.is_typename_at(self.pos + 1) {
                self.pos += 1;
                let ty = self.typename();
                self.expect(")");
                if self.is("{") {
                    self.pos = start; // sizeof (T){...}: the size of a compound literal
                    self.unary().ty
                } else {
                    ty
                }
            } else {
                self.unary().ty
            };
            return self.with_vla_sizes(size_of(&ty, loc));
        }
        if self.consume("_Alignof") || self.consume("__alignof__") || self.consume("__alignof") {
            let ty = if self.is("(") && self.is_typename_at(self.pos + 1) {
                self.pos += 1;
                let ty = self.typename();
                self.expect(")");
                ty
            } else {
                // GNU: alignof applied to an expression; a variable may be over-aligned.
                let e = self.unary();
                let explicit = match &e.kind {
                    NK::LVar(i) => self.locals[*i].align,
                    NK::GVar(name) => self.globals.iter().find(|g| g.name == *name).map_or(0, |g| g.align),
                    _ => 0,
                };
                self.vla_pending.clear();
                return mk(NK::Num(e.ty.align().max(explicit)), ty_ulong(), loc);
            };
            self.vla_pending.clear();
            return mk(NK::Num(ty.align()), ty_ulong(), loc);
        }
        self.postfix()
    }

    /// `n++` / `n--` : (typeof n)((n += d) - d)
    fn inc_dec(&mut self, n: Node, d: i64, loc: Loc) -> Node {
        let ty = n.ty.clone();
        if let NK::Member(base, m) = &n.kind {
            if m.bit_width >= 0 {
                // p = &base, old = p->m, p->m = old + d, old
                let (p, old) = (self.new_lvar("", ptr_to(base.ty.clone())), self.new_lvar("", ty));
                let mem = new_member(new_deref(self.lvar(p, loc), loc), m, loc);
                let set = new_assign(self.lvar(p, loc), new_addr((**base).clone(), loc), loc);
                let save = new_assign(self.lvar(old, loc), mem.clone(), loc);
                let upd = new_assign(mem, new_add(self.lvar(old, loc), num(d, loc), loc), loc);
                return new_comma(set, new_comma(save, new_comma(upd, self.lvar(old, loc), loc), loc), loc);
            }
        }
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
                if !pty.is_record() {
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
        let mut mem = new_comma(bump(self, 2, size8), new_sub(field(self, 2), num(size8, loc), loc), loc);
        if ty.align() >= 16 {
            // Such arguments are 16-byte aligned on the stack.
            let up = new_binary(Op::BitAnd, new_add(new_cast(field(self, 2), ty_ulong()), num(15, loc), loc), num(-16, loc), loc);
            let fix = new_assign(field(self, 2), mk(NK::Cast(bx(up)), ptr_to(ty_char()), loc), loc);
            mem = new_comma(fix, mem, loc);
        }
        let in_mem = mem_class(&ty) || has_ldouble(&ty);
        let from_reg = |p: &Parser, i: usize, step: i64| new_sub(new_add(field(p, 3), new_cast(field(p, i), ty_long()), loc), num(step, loc), loc);
        let fits = |p: &Parser, i: usize, n: i64| {
            let (step, limit) = if i == 0 { (8, 48) } else { (16, 176) };
            new_binary(Op::Le, field(p, i), num(limit - step * n, loc), loc)
        };
        let addr = if ty.size() == 0 {
            field(self, 2) // GNU empty struct: occupies no argument slot
        } else if in_mem {
            mem
        } else if let Type::Struct(_) = &*ty {
            {
                // Small struct: each eightbyte comes from the GP or the SSE save
                // area; assemble them in a temporary.
                let classes: Vec<usize> = [0, 8].iter().filter(|lo| **lo < ty.size()).map(|lo| crate::codegen::has_flonum(&ty, *lo, lo + 8, 0) as usize).collect();
                let buf = self.new_lvar("", array_of(ty_long(), 2));
                let mut copy: Option<Node> = None;
                for (k, &c) in classes.iter().enumerate() {
                    let step = if c == 0 { 8 } else { 16 };
                    let src = new_deref(mk(NK::Cast(bx(from_reg(self, c, step))), ptr_to(ty_long()), loc), loc);
                    let dst = new_deref(new_add(self.lvar(buf, loc), num(k as i64, loc), loc), loc);
                    let e = new_comma(bump(self, c, step), new_assign(dst, src, loc), loc);
                    copy = Some(match copy {
                        Some(prev) => new_comma(prev, e, loc),
                        None => e,
                    });
                }
                let (ngp, nfp) = (classes.iter().filter(|c| **c == 0).count() as i64, classes.iter().filter(|c| **c == 1).count() as i64);
                let cond = mk(NK::LogAnd(bx(fits(self, 0, ngp)), bx(fits(self, 1, nfp))), ty_int(), loc);
                let then = new_comma(copy.unwrap(), mk(NK::Cast(bx(self.lvar(buf, loc))), mem.ty.clone(), loc), loc);
                let pty = mem.ty.clone();
                mk(NK::Cond(bx(cond), bx(then), bx(mem)), pty, loc)
            }
        } else {
            let (i, step) = if ty.is_flonum() { (1, 16) } else { (0, 8) };
            let then = new_comma(bump(self, i, step), from_reg(self, i, step), loc);
            let pty = mem.ty.clone();
            mk(NK::Cond(bx(fits(self, i, 1)), bx(then), bx(mem)), pty, loc)
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
        if self.consume("_Generic") {
            // shortcut: `long`/`long long` (and `char`/`signed char`) are one type in
            // rcc, so the first such association wins.
            self.expect("(");
            let ctrl = decay(&self.assign().ty);
            let (mut chosen, mut dflt) = (None, None);
            while !self.consume(")") {
                self.expect(",");
                if self.consume("default") {
                    self.expect(":");
                    dflt = Some(self.assign());
                    continue;
                }
                let ty = self.typename();
                self.expect(":");
                let e = self.assign();
                // The controlling expression is an rvalue: no qualified type matches it.
                if chosen.is_none() && ty.quals == 0 && same_type(&ctrl, &ty) {
                    chosen = Some(e);
                }
            }
            return chosen.or(dflt).unwrap_or_else(|| err(loc, "_Generic selector matches no association"));
        }
        let t = self.tok().clone();
        match t.kind {
            TK::Num => {
                self.pos += 1;
                match t.nty {
                    NumTy::Float => mk(NK::FNum(t.fval), ty_float(), loc),
                    NumTy::Double => mk(NK::FNum(t.fval), ty_double(), loc),
                    NumTy::LDouble => mk(NK::LNum(t.ival as u64, t.lexp), ty_ldouble(), loc),
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
                        let ap = self.assign();
                        self.expect(")");
                        return mk(NK::Cast(bx(ap)), ty_void(), loc);
                    }
                    "__builtin_prefetch" => {
                        self.expect("(");
                        let mut e = mk(NK::Cast(bx(self.assign())), ty_void(), loc);
                        while !self.consume(")") {
                            self.expect(",");
                            e = new_comma(e, mk(NK::Cast(bx(self.assign())), ty_void(), loc), loc);
                        }
                        return e;
                    }
                    "__builtin_constant_p" => {
                        self.expect("(");
                        let e = self.expr();
                        self.expect(")");
                        let literal = matches!(&e.kind, NK::GVar(name) if name.starts_with(".L.str"));
                        return num((is_const(&e) || literal) as i64, loc);
                    }
                    "__builtin_choose_expr" => {
                        self.expect("(");
                        let c = self.const_expr();
                        self.expect(",");
                        let a = self.assign();
                        self.expect(",");
                        let b = self.assign();
                        self.expect(")");
                        return if c != 0 { a } else { b };
                    }
                    "__builtin_types_compatible_p" => {
                        self.expect("(");
                        let a = self.typename();
                        self.expect(",");
                        let b = self.typename();
                        self.expect(")");
                        return num(same_type(&a, &b) as i64, loc);
                    }
                    "__builtin_classify_type" => {
                        self.expect("(");
                        let ty = if self.is_typename() { self.typename() } else { self.expr().ty };
                        self.expect(")");
                        let class = match &*ty {
                            Type::Void => 0,
                            Type::Struct(sd) => if sd.borrow().is_union { 13 } else { 12 },
                            t if t.is_integer() => 1,
                            t if t.is_flonum() => 8,
                            _ => 5,
                        };
                        return num(class, loc);
                    }
                    "__builtin_frame_address" | "__builtin_return_address" => {
                        self.expect("(");
                        let level = self.const_expr();
                        self.expect(")");
                        let pp = ptr_to(ptr_to(ty_void()));
                        let mut fp = mk(NK::FrameAddr, pp.clone(), loc);
                        for _ in 0..level {
                            fp = mk(NK::Cast(bx(new_deref(fp, loc))), pp.clone(), loc);
                        }
                        if t.s == "__builtin_frame_address" {
                            return mk(NK::Cast(bx(fp)), ptr_to(ty_void()), loc);
                        }
                        return new_deref(new_add(fp, num(1, loc), loc), loc);
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
                    "alloca" | "__builtin_alloca" if self.is("(") => {
                        self.pos += 1;
                        let size = new_cast(self.assign(), ty_ulong());
                        self.expect(")");
                        return mk(NK::Alloca(bx(size)), ptr_to(ty_void()), loc);
                    }
                    "__builtin_expect" => {
                        self.expect("(");
                        let e = self.assign();
                        self.expect(",");
                        let v = self.assign();
                        self.expect(")");
                        if is_const(&v) || e.ty.is_struct() {
                            return e;
                        }
                        // The hint may have side effects: tmp = e, v, tmp
                        let tmp = self.new_lvar("", e.ty.clone());
                        let set = new_assign(self.lvar(tmp, loc), e, loc);
                        return new_comma(set, new_comma(v, self.lvar(tmp, loc), loc), loc);
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
                    Some(VS::Enum(v)) if v == v as i32 as i64 => num(v, loc),
                    Some(VS::Enum(v)) if v == v as u32 as i64 => mk(NK::Num(v), ty_uint(), loc),
                    Some(VS::Enum(v)) => lnum(v, loc),
                    _ if self.is("(") => {
                        // Implicit function declaration (C89); `__builtin_foo` means libc's `foo`.
                        let name = match t.s.strip_prefix("__builtin_") {
                            Some("unreachable" | "trap") => "abort",
                            Some(n) => n,
                            None => {
                                eprintln!("{}:{}: warning: implicit declaration of function '{}'", crate::lex::file_name(loc.0), loc.1, t.s);
                                t.s.as_str()
                            }
                        };
                        if let Some(VS::Global(i)) = self.find_var(name).cloned() {
                            return self.gvar(i, loc); // `__builtin_foo` with `foo` declared
                        }
                        const MATH: &[&str] = &[
                            "fabs", "sqrt", "floor", "ceil", "pow", "sin", "cos", "tan", "exp", "log", "log2", "log10", "copysign",
                            "fmod", "trunc", "round", "rint", "nearbyint", "ldexp", "scalbn", "frexp", "modf", "fmin", "fmax", "atan",
                            "atan2", "asin", "acos", "sinh", "cosh", "tanh", "hypot", "cbrt", "exp2", "expm1", "log1p",
                        ];
                        let ret = match name {
                            "memset" | "memcpy" | "memmove" | "memchr" | "malloc" | "calloc" | "realloc" | "strcpy" | "strncpy"
                            | "strcat" | "strncat" | "strchr" | "strrchr" | "strstr" | "stpcpy" | "mempcpy" | "strdup" => ptr_to(ty_char()),
                            "strlen" | "strspn" | "strcspn" => ty_ulong(),
                            "labs" | "llabs" => ty_long(),
                            "abort" | "exit" | "free" => ty_void(),
                            _ if MATH.contains(&name) => ty_double(),
                            _ if name.strip_suffix('f').is_some_and(|b| MATH.contains(&b)) => ty_float(),
                            _ => ty_int(),
                        };
                        let fty = Ty::new(Type::Func(Rc::new(FuncTy { ret, params: vec![], names: vec![], variadic: false, oldstyle: true })));
                        let i = self.declare_func(name, fty, &Attr::default());
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
