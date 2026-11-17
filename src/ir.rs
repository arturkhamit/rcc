// Intermediate representation of the optimizing back end (-O1 and up):
// three-address code over virtual registers, in basic blocks.
//
// `lower` builds it from the AST of one function. It follows codegen.rs node by
// node: a value has the same representation in a virtual register as it has in
// %rax or %xmm0 there (integers shorter than `int` extended to 32 bits, the
// upper half of a 32-bit value unspecified, structs and arrays as addresses),
// and conversions are the same. A function that uses something not handled
// here is declined, and codegen.rs compiles it.
use crate::parse::{Node, Obj, Op, NK};
use crate::types::*;
use std::collections::{HashMap, HashSet};

pub type V = usize; // virtual register
pub type B = usize; // basic block

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cls {
    Int, // general-purpose register
    Flt, // SSE register
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Base {
    Rbp,
    Sym(String),
    Reg(V),
}

/// A memory operand: base + index * scale + disp.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Addr {
    pub base: Base,
    pub index: Option<(V, u8)>,
    pub disp: i64,
}

/// The second operand of an integer operation; an immediate fits in 32 bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opd {
    V(V),
    Imm(i64),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntKind { I32, I64, U32, U64 }

#[derive(Clone)]
pub enum Ins {
    Entry(Vec<(V, usize)>), // parameters passed in registers: the value and its index among those of its class
    Imm(V, i64),
    FImm(V, u64, u8), // bits; size
    Mov(V, V),
    Bin(Op, V, V, Opd, bool, bool), // dst = a op b; 64-bit; unsigned. A comparison gives 0 or 1.
    Neg(V, V),                      // 64-bit
    Not(V, V),                      // bitwise, 64-bit
    FBin(Op, V, V, V, u8),          // on floats (size 4) or doubles (8); a comparison gives an integer 0 or 1
    FNeg(V, V, u8),
    Ext(V, V, u8, bool),     // extends the low 1 or 2 bytes to 32 bits, or the low 4 to 64; signed
    F2F(V, V, u8),           // float <-> double; the size converted to
    I2F(V, V, u8, IntKind),  // the size converted to; what the integer is
    F2I(V, V, u8, IntKind),  // the size converted from; what the integer is
    /// Size; signed (integers shorter than 4 bytes are extended to 32 bits);
    /// volatile (the load must happen, exactly once).
    Load(V, Addr, u8, bool, bool),
    Store(V, Addr, u8),
    StoreImm(i64, Addr, u8), // an integer constant that fits in 32 bits
    Lea(V, Addr),
    GotAddr(V, String), // the address of a symbol that may be defined elsewhere
    TlsAddr(V, String),
    Copy(V, V, i64), // *dst = *src, that many bytes
    Zero(Addr, i64),
    /// Results (integers come back in %rax, %rdx; floats in %xmm0, %xmm1); the
    /// function, as a value or (if None) by name; arguments; bytes of stack arguments.
    Call(Vec<V>, Option<V>, String, Vec<Arg>, i64),
    Loc(u32, u32),
    // Terminators: one ends every block.
    Jmp(B),
    Br(Op, V, Opd, bool, bool, B, B), // if a op b (64-bit; unsigned) goes to the first block, else to the second
    Switch(V, bool, Vec<(i64, i64, B)>, B), // value; 64-bit; cases lo..=hi; default
    Ret(Vec<V>), // at most two integers and two floats (a small struct is returned in pieces)
}

/// Where an argument of a call goes.
#[derive(Clone)]
pub enum Arg {
    Reg(V),            // the next register of its class
    Stack(V, i64),     // at this offset among the stack arguments
    Copy(V, i64, i64), // the struct at this address, copied to this offset; its size
}

impl Ins {
    /// Calls `f(register, is_definition)` for every virtual register of the instruction.
    pub fn regs(&self, f: &mut dyn FnMut(V, bool)) {
        let addr = |a: &Addr, f: &mut dyn FnMut(V, bool)| {
            if let Base::Reg(v) = a.base {
                f(v, false);
            }
            if let Some((v, _)) = a.index {
                f(v, false);
            }
        };
        let opd = |o: &Opd, f: &mut dyn FnMut(V, bool)| {
            if let Opd::V(v) = o {
                f(*v, false);
            }
        };
        match self {
            Ins::Entry(ps) => ps.iter().for_each(|(v, _)| f(*v, true)),
            Ins::Imm(d, _) | Ins::FImm(d, ..) | Ins::GotAddr(d, _) | Ins::TlsAddr(d, _) => f(*d, true),
            Ins::Mov(d, s) | Ins::Neg(d, s) | Ins::Not(d, s) | Ins::FNeg(d, s, _) | Ins::Ext(d, s, ..) | Ins::F2F(d, s, _) | Ins::I2F(d, s, ..) | Ins::F2I(d, s, ..) => {
                f(*s, false);
                f(*d, true);
            }
            Ins::Bin(_, d, a, b, ..) => {
                f(*a, false);
                opd(b, f);
                f(*d, true);
            }
            Ins::FBin(_, d, a, b, _) => {
                f(*a, false);
                f(*b, false);
                f(*d, true);
            }
            Ins::Load(d, a, ..) | Ins::Lea(d, a) => {
                addr(a, f);
                f(*d, true);
            }
            Ins::Store(s, a, _) => {
                f(*s, false);
                addr(a, f);
            }
            Ins::Copy(d, s, _) => {
                f(*d, false);
                f(*s, false);
            }
            Ins::Zero(a, _) | Ins::StoreImm(_, a, _) => addr(a, f),
            Ins::Call(d, callee, _, args, _) => {
                callee.iter().for_each(|v| f(*v, false));
                args.iter().for_each(|a| match a {
                    Arg::Reg(v) | Arg::Stack(v, _) | Arg::Copy(v, ..) => f(*v, false),
                });
                d.iter().for_each(|v| f(*v, true));
            }
            Ins::Br(_, a, b, ..) => {
                f(*a, false);
                opd(b, f);
            }
            Ins::Switch(v, ..) => f(*v, false),
            Ins::Ret(v) => v.iter().for_each(|v| f(*v, false)),
            Ins::Loc(..) | Ins::Jmp(_) => {}
        }
    }

    /// The same, for changing the registers. (Keep in step with `regs`.)
    pub fn regs_mut(&mut self, f: &mut dyn FnMut(&mut V, bool)) {
        let addr = |a: &mut Addr, f: &mut dyn FnMut(&mut V, bool)| {
            if let Base::Reg(v) = &mut a.base {
                f(v, false);
            }
            if let Some((v, _)) = &mut a.index {
                f(v, false);
            }
        };
        let opd = |o: &mut Opd, f: &mut dyn FnMut(&mut V, bool)| {
            if let Opd::V(v) = o {
                f(v, false);
            }
        };
        match self {
            Ins::Entry(ps) => ps.iter_mut().for_each(|(v, _)| f(v, true)),
            Ins::Imm(d, _) | Ins::FImm(d, ..) | Ins::GotAddr(d, _) | Ins::TlsAddr(d, _) => f(d, true),
            Ins::Mov(d, s) | Ins::Neg(d, s) | Ins::Not(d, s) | Ins::FNeg(d, s, _) | Ins::Ext(d, s, ..) | Ins::F2F(d, s, _) | Ins::I2F(d, s, ..) | Ins::F2I(d, s, ..) => {
                f(s, false);
                f(d, true);
            }
            Ins::Bin(_, d, a, b, ..) => {
                f(a, false);
                opd(b, f);
                f(d, true);
            }
            Ins::FBin(_, d, a, b, _) => {
                f(a, false);
                f(b, false);
                f(d, true);
            }
            Ins::Load(d, a, ..) | Ins::Lea(d, a) => {
                addr(a, f);
                f(d, true);
            }
            Ins::Store(s, a, _) => {
                f(s, false);
                addr(a, f);
            }
            Ins::Copy(d, s, _) => {
                f(d, false);
                f(s, false);
            }
            Ins::Zero(a, _) | Ins::StoreImm(_, a, _) => addr(a, f),
            Ins::Call(d, callee, _, args, _) => {
                callee.iter_mut().for_each(|v| f(v, false));
                args.iter_mut().for_each(|a| match a {
                    Arg::Reg(v) | Arg::Stack(v, _) | Arg::Copy(v, ..) => f(v, false),
                });
                d.iter_mut().for_each(|v| f(v, true));
            }
            Ins::Br(_, a, b, ..) => {
                f(a, false);
                opd(b, f);
            }
            Ins::Switch(v, ..) => f(v, false),
            Ins::Ret(v) => v.iter_mut().for_each(|v| f(v, false)),
            Ins::Loc(..) | Ins::Jmp(_) => {}
        }
    }

    /// The memory operand, if the instruction has one.
    pub fn addr_mut(&mut self) -> Option<&mut Addr> {
        match self {
            Ins::Load(_, a, ..) | Ins::Store(_, a, _) | Ins::StoreImm(_, a, _) | Ins::Lea(_, a) | Ins::Zero(a, _) => Some(a),
            _ => None,
        }
    }

    /// Calls `f` for every block a terminator can go to, to change it.
    pub fn targets_mut(&mut self, f: &mut dyn FnMut(&mut B)) {
        match self {
            Ins::Jmp(t) => f(t),
            Ins::Br(.., t, e) => {
                f(t);
                f(e);
            }
            Ins::Switch(_, _, cases, default) => {
                cases.iter_mut().for_each(|c| f(&mut c.2));
                f(default);
            }
            _ => {}
        }
    }

    /// The blocks a terminator can go to.
    pub fn succs(&self) -> Vec<B> {
        match self {
            Ins::Jmp(b) => vec![*b],
            Ins::Br(.., t, f) => vec![*t, *f],
            Ins::Switch(_, _, cases, default) => cases.iter().map(|c| c.2).chain([*default]).collect(),
            _ => vec![],
        }
    }
}

#[derive(Clone)]
pub struct Func {
    pub blocks: Vec<Vec<Ins>>,
    pub layout: Vec<B>, // the blocks in the order they are emitted; the first is the entry
    pub cls: Vec<Cls>,  // the class of every virtual register
    pub frame: i64,     // bytes of local variables below %rbp
}

/// The members of a bit set.
pub fn bits(set: &[u64]) -> impl Iterator<Item = usize> + '_ {
    set.iter().enumerate().flat_map(|(w, &x)| (0..64).filter(move |b| x >> b & 1 != 0).map(move |b| w * 64 + b))
}

/// The virtual registers live at the start and at the end of every block, as bit sets.
pub fn liveness(f: &Func) -> (Vec<Vec<u64>>, Vec<Vec<u64>>) {
    let (nb, words) = (f.blocks.len(), (f.cls.len() + 63) / 64);
    // gen: read before written in the block; kill: written.
    let (mut gen, mut kill) = (vec![vec![0u64; words]; nb], vec![vec![0u64; words]; nb]);
    for &b in &f.layout {
        let (g, k) = (&mut gen[b], &mut kill[b]);
        for ins in &f.blocks[b] {
            ins.regs(&mut |v, def| {
                if def {
                    k[v / 64] |= 1 << (v % 64);
                } else if k[v / 64] >> (v % 64) & 1 == 0 {
                    g[v / 64] |= 1 << (v % 64);
                }
            });
        }
    }
    let (mut live_in, mut live_out) = (gen.clone(), vec![vec![0u64; words]; nb]);
    loop {
        let mut changed = false;
        for &b in f.layout.iter().rev() {
            let mut out = vec![0u64; words];
            for s in f.blocks[b].last().unwrap().succs() {
                out.iter_mut().zip(&live_in[s]).for_each(|(o, i)| *o |= i);
            }
            if out != live_out[b] {
                changed = true;
                for w in 0..words {
                    live_in[b][w] = gen[b][w] | (out[w] & !kill[b][w]);
                }
                live_out[b] = out;
            }
        }
        if !changed {
            return (live_in, live_out);
        }
    }
}

/// What `lower` needs to know about the translation unit.
pub struct Ctx<'a> {
    pub defined: &'a HashSet<&'a str>, // symbols addressed directly (not through the GOT)
    pub tls: &'a HashSet<&'a str>,
    pub debug: bool,
}

type R<T> = Result<T, &'static str>;

/// Functions that return more than once: after the second return a variable
/// kept in a register would have its old value.
const RETURNS_TWICE: [&str; 7] = ["setjmp", "_setjmp", "__sigsetjmp", "sigsetjmp", "vfork", "getcontext", "savectx"];

/// Calls `f` for the node and everything below it.
pub fn walk<'a>(n: &'a Node, f: &mut dyn FnMut(&'a Node)) {
    f(n);
    let kids: Vec<&Node> = match &n.kind {
        NK::Bin(_, l, r) | NK::LogAnd(l, r) | NK::LogOr(l, r) | NK::Assign(l, r) | NK::Comma(l, r) => vec![l, r],
        NK::Neg(l) | NK::Not(l) | NK::BitNot(l) | NK::Addr(l) | NK::Deref(l) | NK::Cast(l) | NK::Alloca(l) | NK::VaStart(l) | NK::GotoExpr(l) | NK::ExprStmt(l)
        | NK::Member(l, _) | NK::Label(_, l) => vec![l],
        NK::VlaAlloc { size, .. } => vec![size],
        NK::Cond(c, t, e) => vec![c, t, e],
        NK::Call { f: callee, args, .. } => std::iter::once(&**callee).chain(args).collect(),
        NK::StmtExpr(v) | NK::Block(v) => v.iter().collect(),
        NK::Return(e) => e.iter().map(|e| &**e).collect(),
        NK::If(c, t, e) => [&**c, &**t].into_iter().chain(e.iter().map(|e| &**e)).collect(),
        NK::For { init, cond, inc, body, .. } => [init, cond, inc].into_iter().flatten().map(|e| &**e).chain([&**body]).collect(),
        NK::Do { body, cond, .. } | NK::Switch { cond, body, .. } => vec![body, cond],
        NK::AsmOps(_, ops, _) => ops.iter().map(|o| &o.node).collect(),
        NK::Nop | NK::Num(_) | NK::FNum(_) | NK::LNum(..) | NK::LVar(_) | NK::GVar(_) | NK::MemZero(_) | NK::Undef(_) | NK::LabelAddr(_) | NK::FrameAddr | NK::Goto(_)
        | NK::Asm(_) => vec![],
    };
    for k in kids {
        walk(k, f);
    }
}

fn cls_of(ty: &Type) -> R<Cls> {
    match ty {
        Type::Float | Type::Double => Ok(Cls::Flt),
        Type::LDouble => Err("long double"),
        _ => Ok(Cls::Int),
    }
}

/// Types whose value is their address.
fn is_agg(ty: &Type) -> bool {
    matches!(ty, Type::Array(..) | Type::Vla(..) | Type::Struct(_) | Type::Func(_) | Type::Void)
}

fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Bool | Type::Int { .. } | Type::Ptr(_) | Type::Float | Type::Double)
}

/// (size, signed) of a scalar in memory.
fn mem(ty: &Type) -> (u8, bool) {
    match ty {
        Type::Bool => (1, false),
        Type::Int { size, unsigned } => (*size, !*unsigned),
        Type::Float => (4, false),
        _ => (8, false),
    }
}

/// (size, unsigned) of a type as an integer register value (as in codegen.rs).
fn int_info(ty: &Type) -> (u8, bool) {
    match ty {
        Type::Bool => (1, true),
        Type::Int { size, unsigned } => (*size, *unsigned),
        _ => (8, true),
    }
}

struct Lower<'a> {
    f: &'a Obj,
    cx: &'a Ctx<'a>,
    func: Func,
    cur: B,
    labels: HashMap<String, B>,
    var: Vec<Option<V>>, // the locals kept in a virtual register
    offs: Vec<i64>,      // the %rbp offsets of the others
    first_temp: V,       // virtual registers from here on are temporaries, not variables
    no_result: bool,     // the value of the call about to be lowered is not used
    last_loc: (u32, u32),
}

pub fn lower(f: &Obj, cx: &Ctx) -> R<Func> {
    let Type::Func(fty) = &*f.ty else { unreachable!() };
    if f.va_area.is_some() {
        return Err("variadic function");
    }
    if !f.vla_saves.is_empty() {
        return Err("variable-length array");
    }
    let ldouble = |t: &Ty| matches!(**t, Type::LDouble) || t.is_struct() && has_ldouble(t);
    if ldouble(&fty.ret) || f.params.iter().any(|&p| ldouble(&f.locals[p].ty)) {
        return Err("long double");
    }
    let body = f.body.as_ref().unwrap();

    // A scalar local whose address is never taken lives in a virtual register.
    let mut in_memory = vec![false; f.locals.len()];
    walk(body, &mut |n| match &n.kind {
        NK::Addr(l) => {
            if let NK::LVar(i) = l.kind {
                in_memory[i] = true;
            }
        }
        NK::MemZero(i) => in_memory[*i] = true,
        _ => {}
    });
    let mut lo = Lower {
        f,
        cx,
        func: Func { blocks: vec![vec![]], layout: vec![0], cls: vec![], frame: 0 },
        cur: 0,
        labels: HashMap::new(),
        var: vec![None; f.locals.len()],
        offs: vec![0; f.locals.len()],
        first_temp: 0,
        no_result: false,
        last_loc: (0, 0),
    };
    for (i, l) in f.locals.iter().enumerate() {
        if !in_memory[i] && is_scalar(&l.ty) && l.ty.quals & (Q_VOLATILE | Q_ATOMIC) == 0 {
            let v = lo.v(cls_of(&l.ty)?);
            lo.var[i] = Some(v);
        }
    }

    lo.first_temp = lo.func.cls.len();

    // Parameters: those passed in registers arrive through `Entry`; those passed
    // on the stack are loaded from, or simply live, where the caller put them.
    let (mut gp, mut fp, mut top) = (0, 0, 16i64);
    let (mut entry, mut fixups, mut homes) = (vec![], vec![], vec![]);
    let mut on_stack = vec![false; f.locals.len()];
    for &p in &f.params {
        let ty = &f.locals[p].ty;
        let (g0, f0) = (gp, fp);
        if !crate::codegen::in_regs(ty, &mut gp, &mut fp) {
            on_stack[p] = true;
            if ty.align() >= 16 {
                top = align_to(top, 16);
            }
            lo.offs[p] = top;
            if let Some(v) = lo.var[p] {
                let (size, signed) = mem(ty);
                fixups.push(Ins::Load(v, Addr { base: Base::Rbp, index: None, disp: top }, size, signed, false));
            }
            top += align_to(ty.size(), 8).max(8);
        } else if ty.is_struct() {
            // A small struct arrives in up to two registers and is put together in its slot.
            let (mut g, mut fl) = (g0, f0);
            for lo8 in [0, 8] {
                let n = (ty.size() - lo8).min(8);
                if n <= 0 {
                    continue;
                }
                let flt = crate::codegen::has_flonum(ty, lo8, lo8 + 8, 0);
                let v = lo.v(if flt { Cls::Flt } else { Cls::Int });
                entry.push((v, if flt { fl } else { g }));
                if flt { fl += 1 } else { g += 1 }
                homes.push((v, p, lo8, n));
            }
        } else {
            let (cls, (size, signed)) = (cls_of(ty)?, mem(ty));
            let v = match lo.var[p] {
                Some(v) => v,
                None => {
                    let v = lo.v(cls);
                    homes.push((v, p, 0, size as i64)); // kept in memory: stored to its slot
                    v
                }
            };
            entry.push((v, if cls == Cls::Int { g0 } else { f0 }));
            if cls == Cls::Int && size < 4 {
                fixups.push(Ins::Ext(v, v, size, signed)); // the caller need not have extended it
            }
        }
    }
    // Frame offsets of the locals in memory (the layout of codegen.rs).
    let mut bottom = 0i64;
    for (i, l) in f.locals.iter().enumerate() {
        if on_stack[i] || lo.var[i].is_some() {
            continue;
        }
        let size = l.ty.size();
        let align = if matches!(&*l.ty, Type::Array(..)) && size >= 16 { l.ty.align().max(16) } else { l.ty.align() };
        bottom = align_to(bottom + size, align.max(l.align).clamp(1, 16));
        lo.offs[i] = -bottom;
    }
    lo.func.frame = bottom;
    lo.emit(Ins::Entry(entry));
    fixups.into_iter().for_each(|i| lo.emit(i));
    for (v, p, at, size) in homes {
        lo.store_bytes(v, Addr { base: Base::Rbp, index: None, disp: lo.offs[p] + at }, size);
    }

    lo.stmt(body)?;
    if !lo.terminated() {
        if f.name == "main" && !matches!(*fty.ret, Type::Void) {
            let v = lo.iv();
            lo.emit(Ins::Imm(v, 0));
            lo.emit(Ins::Ret(vec![v]));
        } else {
            lo.emit(Ins::Ret(vec![]));
        }
    }

    // Keep the blocks that can be reached: code that a constant condition makes
    // dead must not be emitted (it may refer to functions that do not exist).
    let mut func = lo.func;
    let mut reached = vec![false; func.blocks.len()];
    let mut work = vec![func.layout[0]];
    while let Some(b) = work.pop() {
        if !std::mem::replace(&mut reached[b], true) {
            work.extend(func.blocks[b].last().unwrap().succs());
        }
    }
    let placed: HashSet<B> = func.layout.iter().copied().collect();
    if (0..func.blocks.len()).any(|b| reached[b] && !placed.contains(&b)) {
        return Err("jump to a label that is not defined");
    }
    func.layout.retain(|b| reached[*b]);

    // A variable that may be read before it is written starts as zero: whatever
    // it holds must at least be a value of its type (a `char` within range).
    let entry = func.layout[0];
    let unset: Vec<V> = bits(&liveness(&func).0[entry]).collect();
    for v in unset {
        func.blocks[entry].insert(1, if func.cls[v] == Cls::Int { Ins::Imm(v, 0) } else { Ins::FImm(v, 0, 8) });
    }
    Ok(func)
}

impl<'a> Lower<'a> {
    fn v(&mut self, c: Cls) -> V {
        self.func.cls.push(c);
        self.func.cls.len() - 1
    }
    fn iv(&mut self) -> V {
        self.v(Cls::Int)
    }
    fn block(&mut self) -> B {
        self.func.blocks.push(vec![]);
        self.func.blocks.len() - 1
    }
    fn terminated(&self) -> bool {
        matches!(self.func.blocks[self.cur].last(), Some(Ins::Jmp(_) | Ins::Br(..) | Ins::Switch(..) | Ins::Ret(_)))
    }
    fn emit(&mut self, i: Ins) {
        if self.terminated() {
            let b = self.block(); // code after a jump: unreachable unless a label follows
            self.place(b);
        }
        self.func.blocks[self.cur].push(i);
    }
    fn jump(&mut self, b: B) {
        if !self.terminated() {
            self.func.blocks[self.cur].push(Ins::Jmp(b));
        }
    }
    fn place(&mut self, b: B) {
        self.func.layout.push(b);
        self.cur = b;
    }
    /// Continues in block `b`, which the current block falls into.
    fn start(&mut self, b: B) {
        self.jump(b);
        self.place(b);
    }
    fn label(&mut self, name: &str) -> B {
        if let Some(&b) = self.labels.get(name) {
            return b;
        }
        let b = self.block();
        self.labels.insert(name.to_string(), b);
        b
    }
    /// dst = v. If `v` is a temporary that the last instruction computed, that
    /// instruction computes `dst` directly instead.
    fn mov(&mut self, dst: V, v: V) {
        if v >= self.first_temp {
            if let Some(
                Ins::Imm(d, _) | Ins::FImm(d, ..) | Ins::Bin(_, d, ..) | Ins::Neg(d, _) | Ins::Not(d, _) | Ins::FBin(_, d, ..) | Ins::FNeg(d, ..) | Ins::Ext(d, ..)
                | Ins::F2F(d, ..) | Ins::I2F(d, ..) | Ins::F2I(d, ..) | Ins::Load(d, ..) | Ins::Lea(d, _) | Ins::GotAddr(d, _) | Ins::TlsAddr(d, _),
            ) = self.func.blocks[self.cur].last_mut()
            {
                if *d == v {
                    *d = dst;
                    return;
                }
            }
        }
        self.emit(Ins::Mov(dst, v));
    }
    fn imm(&mut self, c: i64) -> Opd {
        if c == c as i32 as i64 {
            return Opd::Imm(c);
        }
        let t = self.iv();
        self.emit(Ins::Imm(t, c));
        Opd::V(t)
    }
    fn int_bin(&mut self, op: Op, a: V, b: Opd, uns: bool) -> V {
        if matches!((op, b), (Op::Shl | Op::Shr, Opd::Imm(0))) {
            return a;
        }
        let t = self.iv();
        self.emit(Ins::Bin(op, t, a, b, true, uns));
        t
    }
    fn load(&mut self, a: Addr, ty: &Ty) -> R<V> {
        let (v, (size, signed)) = (self.v(cls_of(ty)?), mem(ty));
        self.emit(Ins::Load(v, a, size, signed, ty.quals & Q_VOLATILE != 0));
        Ok(v)
    }
    /// Stores the low `n` bytes of an integer (or a float of 4 or 8 bytes).
    fn store_bytes(&mut self, mut v: V, a: Addr, n: i64) {
        if matches!(n, 1 | 2 | 4 | 8) {
            return self.emit(Ins::Store(v, a, n as u8));
        }
        let mut k = 0;
        for step in [4, 2, 1] {
            if n - k >= step {
                self.emit(Ins::Store(v, Addr { disp: a.disp + k, ..a.clone() }, step as u8));
                v = self.int_bin(Op::Shr, v, Opd::Imm(8 * step), true);
                k += step;
            }
        }
    }
    /// Loads `n` bytes as an integer, reading no byte beyond them.
    fn load_bytes(&mut self, a: Addr, n: i64) -> V {
        let load = |lo: &mut Self, at: i64, size: i64| {
            let t = lo.iv();
            lo.emit(Ins::Load(t, Addr { disp: a.disp + at, ..a.clone() }, size as u8, false, false));
            t
        };
        if matches!(n, 1 | 2 | 4 | 8) {
            return load(self, 0, n);
        }
        let (mut res, mut k) = (None, 0);
        for step in [4, 2, 1] {
            if n - k >= step {
                let t = load(self, k, step);
                let t = self.int_bin(Op::Shl, t, Opd::Imm(8 * k), false);
                res = Some(match res {
                    None => t,
                    Some(r) => self.int_bin(Op::BitOr, r, Opd::V(t), false),
                });
                k += step;
            }
        }
        res.unwrap()
    }
    /// The pieces of a small struct that travel in registers: up to two, each
    /// an integer or a float. `f` gets the offset and size of each piece and
    /// whether it is a float.
    fn pieces(ty: &Ty) -> Vec<(i64, i64, bool)> {
        [0, 8].into_iter().filter(|lo| *lo < ty.size()).map(|lo| (lo, (ty.size() - lo).min(8), crate::codegen::has_flonum(ty, lo, lo + 8, 0))).collect()
    }
    fn load_piece(&mut self, a: &Addr, (at, n, flt): (i64, i64, bool)) -> V {
        let a = Addr { disp: a.disp + at, ..a.clone() };
        if !flt {
            return self.load_bytes(a, n);
        }
        let t = self.v(Cls::Flt);
        self.emit(Ins::Load(t, a, if n == 4 { 4 } else { 8 }, false, false));
        t
    }
    fn lea(&mut self, a: Addr) -> V {
        match a {
            Addr { base: Base::Reg(v), index: None, disp: 0 } => v,
            _ => {
                let v = self.iv();
                self.emit(Ins::Lea(v, a));
                v
            }
        }
    }
    fn fzero(&mut self, size: u8) -> V {
        let z = self.v(Cls::Flt);
        self.emit(Ins::FImm(z, 0, size));
        z
    }
    fn fsize(ty: &Type) -> R<u8> {
        match ty {
            Type::Float => Ok(4),
            Type::Double => Ok(8),
            _ => Err("long double"),
        }
    }

    // ----- addresses -----

    fn addr(&mut self, n: &Node) -> R<Addr> {
        let at = |base| Addr { base, index: None, disp: 0 };
        Ok(match &n.kind {
            NK::LVar(_) if matches!(*n.ty, Type::Vla(..)) => return Err("variable-length array"),
            NK::LVar(i) if self.var[*i].is_some() => return Err("address of a register variable"),
            NK::LVar(i) => Addr { base: Base::Rbp, index: None, disp: self.offs[*i] },
            NK::GVar(name) => {
                if self.cx.tls.contains(name.as_str()) {
                    let v = self.iv();
                    self.emit(Ins::TlsAddr(v, name.clone()));
                    at(Base::Reg(v))
                } else if self.cx.defined.contains(name.as_str()) {
                    at(Base::Sym(name.clone()))
                } else {
                    let v = self.iv();
                    self.emit(Ins::GotAddr(v, name.clone()));
                    at(Base::Reg(v))
                }
            }
            NK::Deref(l) => self.ptr_addr(l)?,
            NK::Comma(l, r) => {
                self.void(l)?;
                self.addr(r)?
            }
            NK::Member(l, m) => {
                let mut a = self.addr(l)?;
                a.disp += m.offset;
                a
            }
            // Struct-valued expressions evaluate to the address of the struct.
            NK::Call { .. } | NK::Assign(..) | NK::Cond(..) | NK::StmtExpr(_) | NK::Cast(_) if n.ty.is_struct() => at(Base::Reg(self.expr(n)?)),
            _ => return Err("not an lvalue"),
        })
    }

    /// The memory a pointer-valued expression points to, as an addressing mode
    /// where possible: `p + 8` and `a + i * 4` need no instruction of their own.
    fn ptr_addr(&mut self, p: &Node) -> R<Addr> {
        match &p.kind {
            NK::Bin(op @ (Op::Add | Op::Sub), l, r) if p.ty.base().is_some() && l.ty.base().is_some() => {
                if let NK::Num(c) = r.kind {
                    let c = if *op == Op::Add { c } else { c.wrapping_neg() };
                    if c == c as i32 as i64 {
                        let mut a = self.ptr_addr(l)?;
                        if a.disp + c == (a.disp + c) as i32 as i64 {
                            a.disp += c;
                            return Ok(a);
                        }
                        let base = self.lea(a);
                        return Ok(Addr { base: Base::Reg(base), index: None, disp: c });
                    }
                }
                if let (Op::Add, NK::Bin(Op::Mul, x, s)) = (*op, &r.kind) {
                    if let NK::Num(s @ (1 | 2 | 4 | 8)) = s.kind {
                        let mut a = self.ptr_addr(l)?;
                        // One index per address, and none next to a symbol (%rip-relative).
                        if a.index.is_some() || matches!(a.base, Base::Sym(_)) {
                            a = Addr { base: Base::Reg(self.lea(a)), index: None, disp: 0 };
                        }
                        a.index = Some((self.expr(x)?, s as u8));
                        return Ok(a);
                    }
                }
                Ok(Addr { base: Base::Reg(self.expr(p)?), index: None, disp: 0 })
            }
            NK::Addr(l) => self.addr(l),
            NK::Cast(l) if matches!(*p.ty, Type::Ptr(_)) && matches!(*l.ty, Type::Ptr(_) | Type::Array(..)) => self.ptr_addr(l),
            _ if matches!(*p.ty, Type::Array(..)) => self.addr(p),
            _ => Ok(Addr { base: Base::Reg(self.expr(p)?), index: None, disp: 0 }),
        }
    }

    // ----- expressions -----

    /// Whether a value is nonzero, as an integer to compare with 0 and the width to compare at.
    fn truth(&mut self, v: V, ty: &Type) -> R<(V, bool)> {
        if ty.is_flonum() {
            let (size, t) = (Self::fsize(ty)?, self.iv());
            let z = self.fzero(size);
            self.emit(Ins::FBin(Op::Ne, t, v, z, size)); // true for a NaN, as it is not zero
            return Ok((t, false));
        }
        Ok((v, !(ty.is_integer() && ty.size() <= 4)))
    }

    fn operand(&mut self, r: &Node) -> R<Opd> {
        match r.kind {
            NK::Num(c) if c == c as i32 as i64 && !r.ty.is_flonum() => Ok(Opd::Imm(c)),
            _ => Ok(Opd::V(self.expr(r)?)),
        }
    }

    fn cast(&mut self, v: V, from: &Type, to: &Type) -> R<V> {
        use Type::*;
        if matches!(to, Void | Struct(_)) || matches!(from, Struct(_) | Void) {
            return Ok(v);
        }
        if matches!(from, LDouble) || matches!(to, LDouble) {
            return Err("long double");
        }
        if let Bool = to {
            let (v, wide) = self.truth(v, from)?;
            let t = self.iv();
            self.emit(Ins::Bin(Op::Ne, t, v, Opd::Imm(0), wide, false));
            return Ok(t);
        }
        Ok(match (from, to) {
            (Float, Float) | (Double, Double) => v,
            (Float | Double, Float | Double) => {
                let t = self.v(Cls::Flt);
                self.emit(Ins::F2F(t, v, Self::fsize(to)?));
                t
            }
            (Float | Double, t) => {
                let (size, unsigned) = int_info(t);
                let kind = if size == 8 && unsigned { IntKind::U64 } else if size == 8 || (size == 4 && unsigned) { IntKind::I64 } else { IntKind::I32 };
                let r = self.iv();
                self.emit(Ins::F2I(r, v, Self::fsize(from)?, kind));
                if size < 4 {
                    self.emit(Ins::Ext(r, r, size, !unsigned));
                }
                r
            }
            (f, Float | Double) => {
                let (size, unsigned) = int_info(f);
                let kind = match (size, unsigned) {
                    (8, true) => IntKind::U64,
                    (8, false) => IntKind::I64,
                    (4, true) => IntKind::U32,
                    _ => IntKind::I32,
                };
                let t = self.v(Cls::Flt);
                self.emit(Ins::I2F(t, v, Self::fsize(to)?, kind));
                t
            }
            (f, t) => {
                let ((fs, fu), (ts, tu)) = (int_info(f), int_info(t));
                if ts < 4 || (ts == 8 && fs < 8) {
                    let r = self.iv();
                    self.emit(if ts < 4 { Ins::Ext(r, v, ts, !tu) } else { Ins::Ext(r, v, 4, !(fs == 4 && fu)) });
                    r
                } else {
                    v
                }
            }
        })
    }

    fn expr(&mut self, n: &Node) -> R<V> {
        Ok(match &n.kind {
            NK::Nop => self.iv(),
            NK::Num(c) => {
                let t = self.iv();
                self.emit(Ins::Imm(t, *c));
                t
            }
            NK::FNum(x) => {
                let t = self.v(Cls::Flt);
                match Self::fsize(&n.ty)? {
                    4 => self.emit(Ins::FImm(t, (*x as f32).to_bits() as u64, 4)),
                    _ => self.emit(Ins::FImm(t, x.to_bits(), 8)),
                }
                t
            }
            NK::Neg(l) => {
                let (v, t) = (self.expr(l)?, self.v(cls_of(&n.ty)?));
                match &*n.ty {
                    Type::Float | Type::Double => self.emit(Ins::FNeg(t, v, Self::fsize(&n.ty)?)),
                    _ => self.emit(Ins::Neg(t, v)),
                }
                t
            }
            NK::LVar(i) if self.var[*i].is_some() => self.var[*i].unwrap(),
            NK::LVar(_) | NK::GVar(_) | NK::Deref(_) => {
                if let (NK::Deref(l), true) = (&n.kind, is_agg(&n.ty)) {
                    return self.expr(l); // the pointer is the address of the array or struct
                }
                let a = self.addr(n)?;
                if is_agg(&n.ty) { self.lea(a) } else { self.load(a, &n.ty)? }
            }
            NK::Member(_, m) => {
                let a = self.addr(n)?;
                if is_agg(&n.ty) {
                    return Ok(self.lea(a));
                }
                let v = self.load(a, &n.ty)?;
                if m.bit_width < 0 {
                    return Ok(v);
                }
                let t = self.int_bin(Op::Shl, v, Opd::Imm(64 - m.bit_width - m.bit_off), false);
                self.int_bin(Op::Shr, t, Opd::Imm(64 - m.bit_width), m.ty.is_unsigned())
            }
            NK::Addr(l) => {
                let a = self.addr(l)?;
                self.lea(a)
            }
            NK::Assign(l, r) => {
                // The value of an assignment is what was stored then, not what the
                // variable holds when the value is used (`j++ + ++j` gives what gcc gives).
                let v = self.assign(n, l, r, true)?;
                if v >= self.first_temp {
                    return Ok(v);
                }
                let t = self.v(self.func.cls[v]);
                self.emit(Ins::Mov(t, v));
                t
            }
            NK::MemZero(i) => {
                self.emit(Ins::Zero(Addr { base: Base::Rbp, index: None, disp: self.offs[*i] }, self.f.locals[*i].ty.size()));
                self.iv()
            }
            NK::Cond(c, t, e) => {
                let (else_b, end_b) = (self.block(), self.block());
                let res = if matches!(*n.ty, Type::Void) { None } else { Some(self.v(cls_of(&n.ty)?)) };
                self.cond_jump(c, else_b, false)?;
                for (arm, next) in [(t, else_b), (e, end_b)] {
                    let v = self.expr(arm)?;
                    if let (Some(res), false) = (res, matches!(*arm.ty, Type::Void)) {
                        self.mov(res, v);
                    }
                    self.jump(end_b);
                    self.place(next);
                }
                match res {
                    Some(res) => res,
                    None => self.iv(),
                }
            }
            NK::Not(l) => {
                let (v, t) = (self.expr(l)?, self.iv());
                if l.ty.is_flonum() {
                    let size = Self::fsize(&l.ty)?;
                    let z = self.fzero(size);
                    self.emit(Ins::FBin(Op::Eq, t, v, z, size)); // false for a NaN
                } else {
                    self.emit(Ins::Bin(Op::Eq, t, v, Opd::Imm(0), !(l.ty.is_integer() && l.ty.size() <= 4), false));
                }
                t
            }
            NK::BitNot(l) => {
                let (v, t) = (self.expr(l)?, self.iv());
                self.emit(Ins::Not(t, v));
                t
            }
            NK::LogAnd(..) | NK::LogOr(..) => {
                let (false_b, end_b, res) = (self.block(), self.block(), self.iv());
                self.cond_jump(n, false_b, false)?;
                self.emit(Ins::Imm(res, 1));
                self.jump(end_b);
                self.place(false_b);
                self.emit(Ins::Imm(res, 0));
                self.start(end_b);
                res
            }
            NK::Cast(l) => {
                let v = self.expr(l)?;
                self.cast(v, &l.ty, &n.ty)?
            }
            NK::Comma(l, r) => {
                self.void(l)?;
                self.expr(r)?
            }
            NK::StmtExpr(stmts) => {
                let mut value = None;
                for (i, s) in stmts.iter().enumerate() {
                    match &s.kind {
                        NK::ExprStmt(e) if i + 1 == stmts.len() => value = Some(self.expr(e)?), // the last expression is the value
                        _ => self.stmt(s)?,
                    }
                }
                match value {
                    Some(v) => v,
                    None => self.iv(),
                }
            }
            NK::Call { f, args, ret_buf } => return self.call(n, f, args, *ret_buf),
            NK::Bin(op, l, r) => {
                if l.ty.is_flonum() {
                    let size = Self::fsize(&l.ty)?;
                    let (b, a) = (self.expr(r)?, self.expr(l)?);
                    let res = match op {
                        Op::Eq | Op::Ne | Op::Lt | Op::Le => self.iv(),
                        Op::Add | Op::Sub | Op::Mul | Op::Div => self.v(Cls::Flt),
                        _ => return Err("invalid operands"),
                    };
                    self.emit(Ins::FBin(*op, res, a, b, size));
                    return Ok(res);
                }
                let (wide, uns) = (l.ty.size() == 8 || l.ty.base().is_some(), l.ty.is_unsigned());
                if let NK::Num(c) = r.kind {
                    if let Some(v) = self.bin_const(*op, l, c, wide, uns)? {
                        return Ok(v);
                    }
                }
                // The right operand first, as codegen.rs (and gcc) do: `x |= f()` sees what f did to x.
                let b = if matches!(op, Op::Div | Op::Mod) { Opd::V(self.expr(r)?) } else { self.operand(r)? };
                let a = self.expr(l)?;
                let res = self.iv();
                self.emit(Ins::Bin(*op, res, a, b, wide, uns));
                res
            }
            NK::LNum(..) => return Err("long double"),
            NK::Alloca(_) | NK::VlaAlloc { .. } => return Err("alloca"),
            NK::VaStart(_) => return Err("variadic function"),
            NK::LabelAddr(_) => return Err("address of a label"),
            NK::FrameAddr => return Err("frame address"),
            _ => return Err("statement used as an expression"),
        })
    }

    /// `l op c` for a constant `c`, where there is a cheaper way than the general one:
    /// nothing at all, or shifts instead of a multiplication or a division.
    fn bin_const(&mut self, op: Op, l: &Node, c: i64, wide: bool, uns: bool) -> R<Option<V>> {
        let bits = if wide { 64 } else { 32 };
        let k = if c > 1 && c & (c - 1) == 0 { c.trailing_zeros() as i64 } else { 0 }; // c == 1 << k
        let bin = |lo: &mut Self, op: Op, a: V, b: i64, uns: bool| {
            let t = lo.iv();
            lo.emit(Ins::Bin(op, t, a, Opd::Imm(b), wide, uns));
            t
        };
        Ok(Some(match op {
            Op::Add | Op::Sub | Op::Shl | Op::Shr | Op::BitOr | Op::BitXor if c == 0 => self.expr(l)?,
            Op::Mul | Op::Div if c == 1 => self.expr(l)?,
            Op::Mul if k > 0 => {
                let a = self.expr(l)?;
                bin(self, Op::Shl, a, k, false)
            }
            Op::Div if k > 0 && uns => {
                let a = self.expr(l)?;
                bin(self, Op::Shr, a, k, true)
            }
            Op::Mod if k > 0 && uns && k < 32 => {
                let a = self.expr(l)?;
                bin(self, Op::BitAnd, a, c - 1, false)
            }
            // The difference of two pointers is a multiple of the element size.
            Op::Div if k > 0 && matches!(&l.kind, NK::Bin(Op::Sub, p, q) if p.ty.base().is_some() && q.ty.base().is_some()) => {
                let a = self.expr(l)?;
                bin(self, Op::Shr, a, k, false)
            }
            // Signed division rounds toward zero: a negative dividend gets c - 1 added first.
            Op::Div | Op::Mod if k > 0 && !uns => {
                let a = self.expr(l)?;
                let sign = bin(self, Op::Shr, a, bits - 1, false);
                let bias = bin(self, Op::Shr, sign, bits - k, true);
                let t = self.iv();
                self.emit(Ins::Bin(Op::Add, t, a, Opd::V(bias), wide, false));
                let q = bin(self, Op::Shr, t, k, false);
                if op == Op::Div {
                    return Ok(Some(q));
                }
                let whole = bin(self, Op::Shl, q, k, false);
                let t = self.iv();
                self.emit(Ins::Bin(Op::Sub, t, a, Opd::V(whole), wide, false));
                t
            }
            _ => return Ok(None),
        }))
    }

    fn call(&mut self, n: &Node, f: &Node, args: &[Node], ret_buf: Option<usize>) -> R<V> {
        let no_result = std::mem::take(&mut self.no_result);
        let ldouble = |t: &Ty| matches!(**t, Type::LDouble) || t.is_struct() && has_ldouble(t);
        if ldouble(&n.ty) || args.iter().any(|a| ldouble(&a.ty)) {
            return Err("long double");
        }
        let name = match &f.kind {
            NK::GVar(name) if matches!(*f.ty, Type::Func(_)) => Some(name.as_str()),
            _ => None,
        };
        if name.is_some_and(|n| RETURNS_TWICE.contains(&n)) {
            return Err("setjmp");
        }
        // Small copies and clears are done in place rather than by libc.
        if let (Some(name @ ("memcpy" | "memmove" | "memset")), [d, s, Node { kind: NK::Num(size), .. }]) = (name, args) {
            let fits = match name {
                "memcpy" => (0..=64).contains(size),
                "memmove" => matches!(size, 1 | 2 | 4 | 8), // one load, then one store: overlap is harmless
                _ => (0..=64).contains(size) && matches!(s.kind, NK::Num(0)),
            };
            if fits {
                let (sv, dv) = (self.expr(s)?, self.expr(d)?);
                match (name, *size) {
                    (_, 0) => {}
                    ("memset", _) => self.emit(Ins::Zero(Addr { base: Base::Reg(dv), index: None, disp: 0 }, *size)),
                    _ => self.emit(Ins::Copy(dv, sv, *size)),
                }
                return Ok(dv);
            }
        }

        // Where each argument goes: registers while they last (a struct takes all
        // of its registers or none), otherwise the stack.
        let ret_mem = mem_class(&n.ty);
        let (mut gp, mut fp, mut stack) = (ret_mem as usize, 0, 0i64);
        let mut places = vec![];
        for a in args {
            if crate::codegen::in_regs(&a.ty, &mut gp, &mut fp) {
                places.push(None);
            } else {
                if a.ty.is_struct() && a.ty.align() >= 16 {
                    stack = align_to(stack, 16);
                }
                places.push(Some(stack));
                stack += if a.ty.is_struct() { align_to(a.ty.size(), 8) } else { 8 };
            }
        }
        // The arguments are evaluated last to first, then the function, as codegen.rs does.
        let mut out: Vec<Vec<Arg>> = vec![];
        for (a, place) in args.iter().zip(&places).rev() {
            let v = self.expr(a)?;
            out.push(match (a.ty.is_struct(), place) {
                (false, None) => vec![Arg::Reg(v)],
                (false, Some(at)) => vec![Arg::Stack(v, *at)],
                (true, Some(at)) => vec![Arg::Copy(v, *at, a.ty.size())],
                (true, None) => {
                    let at = Addr { base: Base::Reg(v), index: None, disp: 0 };
                    Self::pieces(&a.ty).into_iter().map(|p| Arg::Reg(self.load_piece(&at, p))).collect()
                }
            });
        }
        let buf = ret_buf.map(|i| Addr { base: Base::Rbp, index: None, disp: self.offs[i] });
        if ret_mem {
            let p = self.lea(buf.clone().unwrap());
            out.push(vec![Arg::Reg(p)]); // the address of the result is a hidden first argument
        }
        let args: Vec<Arg> = out.into_iter().rev().flatten().collect();
        let callee = match name {
            Some(_) => None,
            None => Some(self.expr(f)?),
        };
        let name = name.unwrap_or_default().to_string();

        if n.ty.is_struct() {
            // In memory the callee has filled the buffer; otherwise the pieces come back in registers.
            let buf = buf.ok_or("struct return")?;
            let pieces = if ret_mem { vec![] } else { Self::pieces(&n.ty) };
            let vs: Vec<V> = pieces.iter().map(|p| self.v(if p.2 { Cls::Flt } else { Cls::Int })).collect();
            self.emit(Ins::Call(vs.clone(), callee, name, args, stack));
            for (v, (at, size, flt)) in vs.into_iter().zip(pieces) {
                let a = Addr { disp: buf.disp + at, ..buf.clone() };
                if flt { self.emit(Ins::Store(v, a, if size == 4 { 4 } else { 8 })) } else { self.store_bytes(v, a, size) }
            }
            return Ok(self.lea(buf));
        }
        if no_result || matches!(*n.ty, Type::Void) {
            self.emit(Ins::Call(vec![], callee, name, args, stack));
            return Ok(self.iv());
        }
        let d = self.v(cls_of(&n.ty)?);
        self.emit(Ins::Call(vec![d], callee, name, args, stack));
        match &*n.ty {
            Type::Bool => self.emit(Ins::Ext(d, d, 1, false)),
            Type::Int { size, unsigned } if *size < 4 => self.emit(Ins::Ext(d, d, *size, !*unsigned)),
            _ => {}
        }
        Ok(d)
    }

    /// `want`: the value of the assignment is used.
    fn assign(&mut self, n: &Node, l: &Node, r: &Node, want: bool) -> R<V> {
        if n.ty.is_struct() {
            let a = self.addr(l)?;
            let (dst, src) = (self.lea(a), self.expr(r)?);
            self.emit(Ins::Copy(dst, src, n.ty.size()));
            return Ok(src);
        }
        if let NK::LVar(i) = l.kind {
            if let Some(x) = self.var[i] {
                let v = self.expr(r)?;
                self.mov(x, v);
                return Ok(x);
            }
        }
        let a = self.addr(l)?;
        let bitfield = matches!(&l.kind, NK::Member(_, m) if m.bit_width >= 0);
        if let (NK::Num(c), false, false, true) = (&r.kind, want, bitfield, n.ty.is_integer() || matches!(*n.ty, Type::Ptr(_))) {
            if *c == *c as i32 as i64 {
                self.emit(Ins::StoreImm(*c, a, mem(&n.ty).0));
                return Ok(0);
            }
        }
        let v = self.expr(r)?;
        if let NK::Member(_, m) = &l.kind {
            if m.bit_width >= 0 {
                // Merge the new value into the bitfield's storage unit.
                let mask = 1u64.checked_shl(m.bit_width as u32).map_or(!0, |x| x - 1);
                let (field, keep) = (self.imm(mask as i64), self.imm(!(mask << m.bit_off) as i64));
                let val = self.int_bin(Op::BitAnd, v, field, false);
                let shifted = self.int_bin(Op::Shl, val, Opd::Imm(m.bit_off), false);
                let unit = self.load(a.clone(), &m.ty)?;
                let rest = self.int_bin(Op::BitAnd, unit, keep, false);
                let merged = self.int_bin(Op::BitOr, rest, Opd::V(shifted), false);
                self.emit(Ins::Store(merged, a, mem(&n.ty).0));
                // The value of the expression is the value stored in the bitfield.
                if m.ty.is_unsigned() {
                    return Ok(val);
                }
                let t = self.int_bin(Op::Shl, val, Opd::Imm(64 - m.bit_width), false);
                return Ok(self.int_bin(Op::Shr, t, Opd::Imm(64 - m.bit_width), false));
            }
        }
        cls_of(&n.ty)?;
        self.emit(Ins::Store(v, a, mem(&n.ty).0));
        Ok(v)
    }

    /// Evaluates an expression whose value is not used.
    fn void(&mut self, n: &Node) -> R<()> {
        // `i++` is `(i += 1) - 1`: as a statement only the assignment matters.
        let int_like = |t: &Type| t.is_integer() || matches!(t, Type::Ptr(_));
        match &n.kind {
            NK::Cast(l) if int_like(&n.ty) && int_like(&l.ty) => self.void(l),
            NK::Bin(Op::Add | Op::Sub, l, r) if int_like(&n.ty) && matches!(r.kind, NK::Num(_)) => self.void(l),
            NK::Call { .. } => {
                self.no_result = true;
                self.expr(n).map(|_| ())
            }
            NK::Assign(l, r) => self.assign(n, l, r, false).map(|_| ()),
            _ => self.expr(n).map(|_| ()),
        }
    }

    /// Jumps to `target` if the truth value of `n` is `when`, and goes on otherwise.
    fn cond_jump(&mut self, n: &Node, target: B, when: bool) -> R<()> {
        match &n.kind {
            NK::Num(c) => {
                if (*c != 0) == when {
                    self.jump(target);
                }
            }
            NK::Not(l) => self.cond_jump(l, target, !when)?,
            NK::Comma(l, r) => {
                self.void(l)?;
                self.cond_jump(r, target, when)?;
            }
            // `a && b` is false as soon as one operand is; `a || b` true likewise.
            NK::LogAnd(l, r) | NK::LogOr(l, r) => {
                if matches!(n.kind, NK::LogAnd(..)) != when {
                    self.cond_jump(l, target, when)?;
                    self.cond_jump(r, target, when)?;
                } else {
                    let skip = self.block();
                    self.cond_jump(l, skip, !when)?;
                    self.cond_jump(r, target, when)?;
                    self.start(skip);
                }
            }
            NK::Bin(op @ (Op::Eq | Op::Ne | Op::Lt | Op::Le), l, r) if !l.ty.is_flonum() => {
                let (wide, uns) = (l.ty.size() == 8 || l.ty.base().is_some(), l.ty.is_unsigned());
                let (b, a) = (self.operand(r)?, self.expr(l)?);
                self.branch(*op, a, b, wide, uns, target, when);
            }
            _ => {
                let v = self.expr(n)?;
                let (v, wide) = self.truth(v, &n.ty)?;
                self.branch(Op::Ne, v, Opd::Imm(0), wide, false, target, when);
            }
        }
        Ok(())
    }

    fn branch(&mut self, op: Op, a: V, b: Opd, wide: bool, uns: bool, target: B, when: bool) {
        let next = self.block();
        let (t, f) = if when { (target, next) } else { (next, target) };
        self.emit(Ins::Br(op, a, b, wide, uns, t, f));
        self.place(next);
    }

    // ----- statements -----

    fn stmt(&mut self, n: &Node) -> R<()> {
        if self.cx.debug && n.loc != self.last_loc && !matches!(n.kind, NK::Block(_) | NK::Nop | NK::Undef(_)) {
            self.emit(Ins::Loc(n.loc.0, n.loc.1));
            self.last_loc = n.loc;
        }
        match &n.kind {
            NK::Nop => {}
            // A variable without an initializer starts its life here, with some
            // value of its type: one that was left from before (the last turn of
            // a loop, say) would keep the register busy all the way from there.
            NK::Undef(i) => {
                if let Some(v) = self.var[*i] {
                    self.emit(if self.func.cls[v] == Cls::Int { Ins::Imm(v, 0) } else { Ins::FImm(v, 0, 8) });
                }
            }
            NK::Return(e) => {
                let vs = match e {
                    None => vec![],
                    Some(e) if !e.ty.is_struct() => vec![self.expr(e)?],
                    Some(e) if mem_class(&e.ty) => {
                        // Copied to the caller's buffer, whose address is the hidden first parameter.
                        let hidden = self.f.params[0];
                        let src = self.expr(e)?;
                        let dst = match self.var[hidden] {
                            Some(v) => v,
                            None => self.load(Addr { base: Base::Rbp, index: None, disp: self.offs[hidden] }, &ty_ulong())?,
                        };
                        self.emit(Ins::Copy(dst, src, e.ty.size()));
                        vec![dst]
                    }
                    Some(e) => {
                        let at = Addr { base: Base::Reg(self.expr(e)?), index: None, disp: 0 };
                        Self::pieces(&e.ty).into_iter().map(|p| self.load_piece(&at, p)).collect()
                    }
                };
                self.emit(Ins::Ret(vs));
            }
            NK::If(c, t, e) => {
                let end_b = self.block();
                let else_b = if e.is_some() { self.block() } else { end_b };
                self.cond_jump(c, else_b, false)?;
                self.stmt(t)?;
                if let Some(e) = e {
                    self.jump(end_b);
                    self.place(else_b);
                    self.stmt(e)?;
                }
                self.start(end_b);
            }
            NK::For { init, cond, inc, body, brk, cont } => {
                if let Some(i) = init {
                    self.stmt(i)?;
                }
                let (begin, brk, cont) = (self.block(), self.label(brk), self.label(cont));
                // The condition is tested once before the loop and then at its end:
                // one jump per iteration instead of two. A condition that defines
                // labels (in a statement expression) cannot be written twice.
                let mut twice = true;
                if let Some(c) = cond {
                    walk(c, &mut |n| twice &= !matches!(n.kind, NK::StmtExpr(_)));
                }
                match cond {
                    Some(c) if twice => self.cond_jump(c, brk, false)?,
                    _ => {}
                }
                self.start(begin);
                match cond {
                    Some(c) if !twice => self.cond_jump(c, brk, false)?,
                    _ => {}
                }
                self.stmt(body)?;
                self.start(cont);
                if let Some(i) = inc {
                    self.void(i)?;
                }
                match cond {
                    Some(c) if twice => self.cond_jump(c, begin, true)?,
                    _ => self.jump(begin),
                }
                self.start(brk);
            }
            NK::Do { body, cond, brk, cont } => {
                let (begin, brk, cont) = (self.block(), self.label(brk), self.label(cont));
                self.start(begin);
                self.stmt(body)?;
                self.start(cont);
                self.cond_jump(cond, begin, true)?;
                self.start(brk);
            }
            NK::Switch { cond, body, cases, default, brk } => {
                let v = self.expr(cond)?;
                let cases: Vec<(i64, i64, B)> = cases.iter().map(|(lo, hi, l)| (*lo, *hi, self.label(l))).collect();
                let (dflt, brk) = (self.label(default.as_ref().unwrap_or(brk)), self.label(brk));
                self.emit(Ins::Switch(v, cond.ty.size() == 8, cases, dflt));
                self.stmt(body)?;
                self.start(brk);
            }
            NK::Label(l, body) => {
                let b = self.label(l);
                self.start(b);
                self.stmt(body)?;
            }
            NK::Goto(l) => {
                let b = self.label(l);
                self.jump(b);
            }
            NK::Block(v) => {
                for s in v {
                    self.stmt(s)?;
                }
            }
            NK::ExprStmt(e) => self.void(e)?,
            NK::Asm(_) | NK::AsmOps(..) => return Err("asm"),
            NK::GotoExpr(_) => return Err("computed goto"),
            _ => return Err("invalid statement"),
        }
        Ok(())
    }
}
