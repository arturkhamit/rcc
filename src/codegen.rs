// x86-64 System V code generator. A simple stack machine in the style of
// chibicc: every expression leaves its value in %rax (or %xmm0 for floats).
use crate::lex::error_at;
use crate::parse::{Node, Obj, Op, NK};
use crate::types::*;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

const ARG8: [&str; 6] = ["%dil", "%sil", "%dl", "%cl", "%r8b", "%r9b"];
const ARG16: [&str; 6] = ["%di", "%si", "%dx", "%cx", "%r8w", "%r9w"];
const ARG32: [&str; 6] = ["%edi", "%esi", "%edx", "%ecx", "%r8d", "%r9d"];
const ARG64: [&str; 6] = ["%rdi", "%rsi", "%rdx", "%rcx", "%r8", "%r9"];

macro_rules! emit {
    ($s:expr, $($arg:tt)*) => { writeln!($s.out, $($arg)*).unwrap() };
}

/// True if every scalar in bytes [lo, hi) of the type is a float or double.
pub fn has_flonum(ty: &Ty, lo: i64, hi: i64, off: i64) -> bool {
    match &**ty {
        Type::Struct(s) => s.borrow().members.iter().all(|m| has_flonum(&m.ty, lo, hi, off + m.offset)),
        Type::Array(b, n) => (0..*n).all(|i| has_flonum(b, lo, hi, off + b.size() * i)),
        Type::Float | Type::Double => true,
        _ => off + ty.size() <= lo || hi <= off,
    }
}

/// Decides whether an argument of the given type is passed in registers,
/// updating the counts of used general-purpose and SSE registers.
fn in_regs(ty: &Ty, gp: &mut usize, fp: &mut usize) -> bool {
    let (ng, nf) = match &**ty {
        Type::LDouble => return false,
        Type::Struct(_) => {
            if mem_class(ty) || has_ldouble(ty) {
                return false;
            }
            if ty.size() == 0 {
                return true; // GNU empty struct: takes no register and no stack
            }
            let f1 = has_flonum(ty, 0, 8, 0) as usize;
            let f2 = if ty.size() > 8 { has_flonum(ty, 8, 16, 0) as usize } else { 0 };
            let n = if ty.size() > 8 { 2 } else { 1 };
            (n - f1 - f2, f1 + f2)
        }
        Type::Float | Type::Double => (0, 1),
        _ => (1, 0),
    };
    if *gp + ng > 6 || *fp + nf > 8 {
        return false;
    }
    *gp += ng;
    *fp += nf;
    true
}

/// (size, unsigned) of a type as an integer register value.
fn int_info(ty: &Type) -> (i64, bool) {
    match ty {
        Type::Bool => (1, true),
        Type::Int { size, unsigned } => (*size as i64, *unsigned),
        _ => (8, true),
    }
}

struct Gen<'a> {
    out: String,
    depth: i64,
    cnt: usize,
    defined: HashSet<&'a str>,
    tls: HashSet<&'a str>,
    func: Option<&'a Obj>,
    offs: Vec<i64>,
    va: (usize, usize, i64), // gp regs used, fp regs used, start of stack args
    has_asm: bool,           // the current function contains an asm statement
}

/// `pic`: the code may go into a shared library, where a global symbol can be
/// preempted and so must be reached through the GOT even if defined here.
pub fn codegen(globals: &[Obj], top_asm: &[String], pic: bool) -> String {
    let mut g = Gen { out: String::new(), depth: 0, cnt: 0, defined: HashSet::new(), tls: HashSet::new(), func: None, offs: vec![], va: (0, 0, 0), has_asm: false };
    g.tls = globals.iter().filter(|o| o.is_tls).map(|o| o.name.as_str()).collect();
    g.defined = globals.iter().filter(|o| o.is_def && (o.is_static || !pic)).map(|o| o.name.as_str()).collect();

    // Every function is generated, but a static one is kept only if code that is
    // kept refers to it. Going by the generated text rather than by the source
    // lets calls in eliminated dead code drop their callees too.
    let mut texts: HashMap<&str, String> = HashMap::new();
    for o in globals.iter().filter(|o| o.is_def && o.is_func) {
        g.emit_func(o);
        texts.insert(&o.name, std::mem::take(&mut g.out));
    }
    let statics: HashSet<&str> = globals.iter().filter(|o| o.is_def && o.is_func && o.is_static).map(|o| o.name.as_str()).collect();
    let mut live: HashSet<&str> = HashSet::new();
    let mut work: Vec<&str> = Vec::new();
    for o in globals.iter().filter(|o| o.is_def) {
        if !o.is_func {
            // A relocation against a label (`&&l` in a static table) keeps its function.
            work.extend(o.relocs.iter().map(|r| r.label.strip_prefix(".L.u.").and_then(|l| l.split('.').next()).unwrap_or(&r.label)));
        } else if !o.is_static || o.sym.ctor || o.sym.dtor {
            work.push(&o.name);
        }
    }
    work.extend(globals.iter().filter_map(|o| o.sym.alias.as_deref()));
    while let Some(name) = work.pop() {
        if !live.insert(name) {
            continue;
        }
        if let Some(text) = texts.get(name) {
            let words = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'));
            work.extend(words.filter_map(|w| statics.get(w).copied()));
        }
    }

    for text in top_asm {
        emit!(g, "{}", text);
    }
    for o in globals {
        if let Some(target) = &o.sym.alias {
            emit!(g, "  .{} {}\n  .set {}, {}", if o.sym.weak { "weak" } else if o.is_static { "local" } else { "globl" }, o.name, o.name, target);
        } else if o.sym.weak && !o.is_def {
            emit!(g, "  .weak {}", o.name); // may stay undefined
        }
    }
    for o in globals.iter().filter(|o| o.is_def && !o.is_func) {
        g.emit_data(o);
    }
    for o in globals.iter().filter(|o| o.is_def && o.is_func && live.contains(o.name.as_str())) {
        g.out.push_str(&texts[o.name.as_str()]);
    }
    emit!(g, "  .section .note.GNU-stack,\"\",@progbits");
    g.out
}

impl<'a> Gen<'a> {
    fn next(&mut self) -> usize {
        self.cnt += 1;
        self.cnt
    }
    fn push(&mut self) {
        emit!(self, "  push %rax");
        self.depth += 1;
    }
    fn pop(&mut self, reg: &str) {
        emit!(self, "  pop {}", reg);
        self.depth -= 1;
    }
    fn pushf(&mut self) {
        emit!(self, "  sub $8, %rsp");
        emit!(self, "  movsd %xmm0, (%rsp)");
        self.depth += 1;
    }
    fn popf(&mut self, reg: usize) {
        emit!(self, "  movsd (%rsp), %xmm{}", reg);
        emit!(self, "  add $8, %rsp");
        self.depth -= 1;
    }

    fn pushld(&mut self) {
        emit!(self, "  sub $16, %rsp\n  fstpt (%rsp)");
        self.depth += 2;
    }
    fn popld(&mut self) {
        emit!(self, "  fldt (%rsp)\n  add $16, %rsp");
        self.depth -= 2;
    }
    /// Evaluates an expression whose value is not used. A long double must be
    /// popped: the x87 register stack holds only eight values.
    fn gen_void(&mut self, n: &Node) {
        // `i++` is `(i += 1) - 1`: as a statement only the assignment matters.
        let int_like = |t: &Type| t.is_integer() || matches!(t, Type::Ptr(_));
        match &n.kind {
            NK::Cast(l) if int_like(&n.ty) && int_like(&l.ty) => return self.gen_void(l),
            NK::Bin(Op::Add | Op::Sub, l, r) if int_like(&n.ty) && matches!(r.kind, NK::Num(_)) => return self.gen_void(l),
            _ => {}
        }
        self.gen_expr(n);
        if let Type::LDouble = *n.ty {
            emit!(self, "  fstp %st(0)");
        }
    }

    fn emit_data(&mut self, o: &Obj) {
        let size = o.ty.size();
        let align = if matches!(&*o.ty, Type::Array(..)) && size >= 16 { o.ty.align().max(16) } else { o.ty.align() };
        let align = align.max(o.align);
        if !o.name.starts_with(".L") {
            self.emit_binding(o);
        }
        if let Some(sec) = &o.sym.section {
            emit!(self, "  .section {},\"aw\",@progbits\n  .balign {}\n{}:", sec, align, o.name);
            match &o.init {
                Some(_) => self.emit_bytes(o),
                None => emit!(self, "  .zero {}", size),
            }
            return;
        }
        if o.init.is_none() {
            emit!(self, "  {}\n  .balign {}\n{}:\n  .zero {}", if o.is_tls { ".section .tbss,\"awT\",@nobits" } else { ".bss" }, align, o.name, size);
            return;
        }
        emit!(self, "  {}\n  .balign {}\n{}:", if o.is_tls { ".section .tdata,\"awT\",@progbits" } else { ".data" }, align, o.name);
        self.emit_bytes(o);
    }

    fn emit_binding(&mut self, o: &Obj) {
        emit!(self, "  .{} {}", if o.sym.weak { "weak" } else if o.is_static { "local" } else { "globl" }, o.name);
        if o.sym.hidden {
            emit!(self, "  .hidden {}", o.name);
        }
    }

    fn emit_bytes(&mut self, o: &Obj) {
        let data = o.init.as_ref().unwrap();
        let (mut pos, mut ri) = (0usize, 0usize);
        while pos < data.len() {
            if ri < o.relocs.len() && o.relocs[ri].offset as usize == pos {
                emit!(self, "  .quad {}{:+}", o.relocs[ri].label, o.relocs[ri].addend);
                pos += 8;
                ri += 1;
                continue;
            }
            let end = if ri < o.relocs.len() { (o.relocs[ri].offset as usize).min(data.len()) } else { data.len() };
            let zero = data[pos] == 0;
            let mut n = pos;
            while n < end && (data[n] == 0) == zero && (zero || n - pos < 16) {
                n += 1;
            }
            if zero {
                emit!(self, "  .zero {}", n - pos);
            } else {
                let bytes: Vec<String> = data[pos..n].iter().map(|b| b.to_string()).collect();
                emit!(self, "  .byte {}", bytes.join(","));
            }
            pos = n;
        }
    }

    fn emit_func(&mut self, f: &'a Obj) {
        let Type::Func(fty) = &*f.ty else { unreachable!() };
        self.func = Some(f);
        self.has_asm = false;
        let start = self.out.len();

        // Assign stack offsets to parameters and locals.
        self.offs = vec![0; f.locals.len()];
        let (mut gp, mut fp, mut top) = (0, 0, 16i64);
        let mut on_stack = vec![false; f.locals.len()];
        for &p in &f.params {
            let ty = &f.locals[p].ty;
            if !in_regs(ty, &mut gp, &mut fp) {
                on_stack[p] = true;
                if ty.align() >= 16 {
                    top = align_to(top, 16);
                }
                self.offs[p] = top;
                top += align_to(ty.size(), 8);
            }
        }
        self.va = (gp, fp, top);
        let mut bottom = 0i64;
        for (i, l) in f.locals.iter().enumerate() {
            if on_stack[i] {
                continue;
            }
            let size = l.ty.size();
            let align = if matches!(&*l.ty, Type::Array(..)) && size >= 16 { l.ty.align().max(16) } else { l.ty.align() };
            // shortcut: the frame is 16-byte aligned, so a local cannot be aligned
            // more strictly; realign %rsp in the prologue if that is ever needed.
            bottom = align_to(bottom + size, align.max(l.align).clamp(1, 16));
            self.offs[i] = -bottom;
        }

        self.emit_binding(f);
        match &f.sym.section {
            Some(sec) => emit!(self, "  .section {},\"ax\",@progbits", sec),
            None => emit!(self, "  .text"),
        }
        if f.align > 1 {
            emit!(self, "  .balign {}", f.align);
        }
        emit!(self, "  .type {}, @function\n{}:", f.name, f.name);
        emit!(self, "  push %rbp\n  mov %rsp, %rbp\n  sub ${}, %rsp", align_to(bottom, 16));

        for &v in &f.vla_saves {
            emit!(self, "  movq $0, {}(%rbp)", self.offs[v]);
        }
        if let Some(va) = f.va_area {
            // Register save area for va_arg.
            let off = self.offs[va];
            for (i, r) in ARG64.iter().enumerate() {
                emit!(self, "  mov {}, {}(%rbp)", r, off + 8 * i as i64);
            }
            for i in 0..8 {
                emit!(self, "  movsd %xmm{}, {}(%rbp)", i, off + 48 + 16 * i);
            }
        }

        // Save register-passed parameters to their stack slots.
        let (mut gp, mut fp) = (0, 0);
        for &p in &f.params {
            if on_stack[p] {
                continue;
            }
            let (ty, off) = (&f.locals[p].ty, self.offs[p]);
            match &**ty {
                Type::Struct(_) => {
                    let size = ty.size();
                    for (lo, n) in [(0, size.min(8)), (8, size - 8)] {
                        if n <= 0 {
                            continue;
                        }
                        if has_flonum(ty, lo, lo + 8, 0) {
                            self.store_fp(fp, off + lo, n);
                            fp += 1;
                        } else {
                            self.store_gp(gp, off + lo, n);
                            gp += 1;
                        }
                    }
                }
                Type::Float | Type::Double => {
                    self.store_fp(fp, off, ty.size());
                    fp += 1;
                }
                _ => {
                    self.store_gp(gp, off, ty.size());
                    gp += 1;
                }
            }
        }

        self.depth = 0;
        self.gen_stmt(f.body.as_ref().unwrap());
        assert!(self.depth == 0, "stack depth mismatch in {}", f.name);
        if f.name == "main" && !matches!(*fty.ret, Type::Void) {
            emit!(self, "  mov $0, %eax");
        }
        emit!(self, ".L.return.{}:\n  mov %rbp, %rsp\n  pop %rbp\n  ret", f.name);
        emit!(self, "  .size {}, .-{}", f.name, f.name);
        if !self.has_asm {
            let text = crate::opt::remove_dead_code(&self.out[start..]);
            self.out.truncate(start);
            self.out.push_str(&text);
        }
        for (on, sec) in [(f.sym.ctor, "init"), (f.sym.dtor, "fini")] {
            if on {
                emit!(self, "  .section .{}_array,\"aw\",@{}_array\n  .balign 8\n  .quad {}", sec, sec, f.name);
            }
        }
    }

    fn store_fp(&mut self, r: usize, off: i64, size: i64) {
        if size == 4 {
            emit!(self, "  movss %xmm{}, {}(%rbp)", r, off);
        } else {
            emit!(self, "  movsd %xmm{}, {}(%rbp)", r, off);
        }
    }

    fn store_gp(&mut self, r: usize, off: i64, size: i64) {
        match size {
            1 => emit!(self, "  mov {}, {}(%rbp)", ARG8[r], off),
            2 => emit!(self, "  mov {}, {}(%rbp)", ARG16[r], off),
            4 => emit!(self, "  mov {}, {}(%rbp)", ARG32[r], off),
            8 => emit!(self, "  mov {}, {}(%rbp)", ARG64[r], off),
            _ => {
                for i in 0..size {
                    emit!(self, "  mov {}, {}(%rbp)", ARG8[r], off + i);
                    emit!(self, "  shr $8, {}", ARG64[r]);
                }
            }
        }
    }

    // ----- expressions -----

    fn gen_addr(&mut self, n: &Node) {
        match &n.kind {
            // A VLA variable holds the address of its storage.
            NK::LVar(i) if matches!(*n.ty, Type::Vla(..)) => emit!(self, "  mov {}(%rbp), %rax", self.offs[*i]),
            NK::LVar(i) => emit!(self, "  lea {}(%rbp), %rax", self.offs[*i]),
            NK::GVar(name) if self.tls.contains(name.as_str()) => {
                // Initial-exec TLS model: valid in executables and in libraries loaded at startup.
                emit!(self, "  mov {}@gottpoff(%rip), %rax\n  add %fs:0, %rax", name);
            }
            NK::GVar(name) => {
                if self.defined.contains(name.as_str()) {
                    emit!(self, "  lea {}(%rip), %rax", name);
                } else {
                    emit!(self, "  mov {}@GOTPCREL(%rip), %rax", name);
                }
            }
            NK::Deref(l) => self.gen_expr(l),
            NK::Comma(l, r) => {
                self.gen_void(l);
                self.gen_addr(r);
            }
            NK::Member(l, m) => {
                self.gen_addr(l);
                if m.offset != 0 {
                    emit!(self, "  add ${}, %rax", m.offset);
                }
            }
            // Struct-valued expressions evaluate to the address of the struct.
            NK::Call { .. } | NK::Assign(..) | NK::Cond(..) | NK::StmtExpr(_) | NK::Cast(_) if n.ty.is_struct() => self.gen_expr(n),
            _ => error_at(n.loc.0, n.loc.1, "not an lvalue"),
        }
    }

    fn load(&mut self, ty: &Ty) {
        self.load_from(ty, "(%rax)");
    }

    /// Loads a value of the given type from a memory operand.
    fn load_from(&mut self, ty: &Ty, src: &str) {
        match &**ty {
            // Arrays, structs and functions are represented by their address.
            Type::Array(..) | Type::Vla(..) | Type::Struct(_) | Type::Func(_) | Type::Void => {
                if src != "(%rax)" {
                    emit!(self, "  lea {}, %rax", src);
                }
            }
            Type::Float => emit!(self, "  movss {}, %xmm0", src),
            Type::Double => emit!(self, "  movsd {}, %xmm0", src),
            Type::LDouble => emit!(self, "  fldt {}", src),
            Type::Bool => emit!(self, "  movzbl {}, %eax", src),
            Type::Int { size: 1, unsigned } => emit!(self, "  mov{}bl {}, %eax", if *unsigned { 'z' } else { 's' }, src),
            Type::Int { size: 2, unsigned } => emit!(self, "  mov{}wl {}, %eax", if *unsigned { 'z' } else { 's' }, src),
            Type::Int { size: 4, .. } => emit!(self, "  mov {}, %eax", src),
            _ => emit!(self, "  mov {}, %rax", src),
        }
    }

    /// Stores %rax (or %xmm0) to the address on the stack top.
    fn store(&mut self, ty: &Ty) {
        self.pop("%rdi");
        self.store_to(ty, "(%rdi)");
    }

    fn store_to(&mut self, ty: &Ty, dst: &str) {
        match &**ty {
            Type::Struct(_) => {
                if dst != "(%rdi)" {
                    emit!(self, "  lea {}, %rdi", dst);
                }
                let size = ty.size();
                if size <= 64 {
                    // Small structs are copied through %rcx.
                    let mut k = 0;
                    for (step, reg) in [(8, "%rcx"), (4, "%ecx"), (2, "%cx"), (1, "%cl")] {
                        while size - k >= step {
                            emit!(self, "  mov {}(%rax), {}\n  mov {}, {}(%rdi)", k, reg, reg, k);
                            k += step;
                        }
                    }
                } else {
                    emit!(self, "  mov %rax, %rsi\n  mov ${}, %ecx\n  rep movsb", size);
                }
            }
            Type::Float => emit!(self, "  movss %xmm0, {}", dst),
            Type::Double => emit!(self, "  movsd %xmm0, {}", dst),
            Type::LDouble => emit!(self, "  fld %st(0)\n  fstpt {}", dst), // the value stays on the x87 stack
            _ => match ty.size() {
                1 => emit!(self, "  mov %al, {}", dst),
                2 => emit!(self, "  mov %ax, {}", dst),
                4 => emit!(self, "  mov %eax, {}", dst),
                _ => emit!(self, "  mov %rax, {}", dst),
            },
        }
    }

    /// Memory operand of a variable that can be addressed directly.
    fn direct(&self, n: &Node) -> Option<String> {
        match &n.kind {
            NK::LVar(_) if matches!(*n.ty, Type::Vla(..)) => None,
            NK::LVar(i) => Some(format!("{}(%rbp)", self.offs[*i])),
            NK::GVar(name) if self.defined.contains(name.as_str()) && !self.tls.contains(name.as_str()) => Some(format!("{}(%rip)", name)),
            NK::Member(l, m) if m.bit_width < 0 => {
                let base = self.direct(l)?;
                let (disp, reg) = base.split_once('(')?;
                Some(match reg {
                    "%rbp)" => format!("{}(%rbp)", disp.parse::<i64>().ok()? + m.offset),
                    _ => format!("{}+{}({}", disp, m.offset, reg),
                })
            }
            _ => None,
        }
    }

    /// An instruction that loads an integer operand without side effects into a
    /// register, touching no other register. None if `n` is not that simple.
    fn simple_load(&self, n: &Node, r64: &str, r32: &str) -> Option<String> {
        let int_like = |t: &Type| t.is_integer() || matches!(t, Type::Ptr(_));
        // Loads the variable at `m` extended to 64 bits (or to 32, which is enough for `want`).
        let load = |ty: &Type, m: &str, want: i64| -> Option<String> {
            Some(match ty {
                Type::Bool | Type::Int { size: 1, unsigned: true } => format!("  movzbl {}, {}", m, r32),
                Type::Int { size: 1, .. } => format!("  movsbq {}, {}", m, r64),
                Type::Int { size: 2, unsigned: true } => format!("  movzwl {}, {}", m, r32),
                Type::Int { size: 2, .. } => format!("  movswq {}, {}", m, r64),
                Type::Int { size: 4, unsigned: false } if want == 8 => format!("  movslq {}, {}", m, r64),
                Type::Int { size: 4, .. } => format!("  mov {}, {}", m, r32),
                Type::Int { .. } | Type::Ptr(_) => format!("  mov {}, {}", m, r64),
                Type::Array(..) | Type::Func(_) => format!("  lea {}, {}", m, r64),
                _ => return None,
            })
        };
        match &n.kind {
            NK::Num(v) if int_like(&n.ty) => Some(if *v >= 0 && *v == *v as i32 as i64 { format!("  mov ${}, {}", v, r32) } else { format!("  mov ${}, {}", v, r64) }),
            NK::LVar(_) | NK::GVar(_) | NK::Member(..) => load(&n.ty, &self.direct(n)?, n.ty.size()),
            NK::Addr(l) => Some(format!("  lea {}, {}", self.direct(l)?, r64)),
            // A widening conversion of a variable.
            NK::Cast(l) if int_like(&n.ty) && l.ty.is_integer() && !matches!(*n.ty, Type::Bool) && n.ty.size() >= l.ty.size() => match &l.kind {
                NK::LVar(_) | NK::GVar(_) | NK::Member(..) => load(&l.ty, &self.direct(l)?, n.ty.size()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The same for a float or double operand and an SSE register (may use %rax).
    fn simple_fload(&self, n: &Node, reg: usize) -> Option<String> {
        match (&n.kind, &*n.ty) {
            (NK::FNum(v), Type::Float) => Some(format!("  mov ${}, %eax\n  movd %eax, %xmm{}", (*v as f32).to_bits(), reg)),
            (NK::FNum(v), Type::Double) => Some(format!("  mov ${}, %rax\n  movq %rax, %xmm{}", v.to_bits() as i64, reg)),
            (NK::LVar(_) | NK::GVar(_) | NK::Member(..), Type::Float) => Some(format!("  movss {}, %xmm{}", self.direct(n)?, reg)),
            (NK::LVar(_) | NK::GVar(_) | NK::Member(..), Type::Double) => Some(format!("  movsd {}, %xmm{}", self.direct(n)?, reg)),
            _ => None,
        }
    }

    /// Evaluates the operands of an integer operation: the left one into %rax, the
    /// right one into %rdi, or returned as an immediate. The stack is used only
    /// when neither operand is simple.
    fn gen_operands(&mut self, op: Op, l: &Node, r: &Node) -> Option<i64> {
        if let NK::Num(v) = r.kind {
            if !matches!(op, Op::Div | Op::Mod) && v == v as i32 as i64 && !r.ty.is_flonum() {
                self.gen_expr(l);
                return Some(v);
            }
        }
        let commutative = matches!(op, Op::Add | Op::Mul | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Eq | Op::Ne);
        if let Some(ins) = self.simple_load(r, "%rdi", "%edi") {
            self.gen_expr(l);
            emit!(self, "{}", ins);
        } else if let (true, Some(ins)) = (commutative, self.simple_load(l, "%rdi", "%edi")) {
            self.gen_expr(r); // operands swapped
            emit!(self, "{}", ins);
        } else if self.simple_load(l, "%rdi", "%edi").is_some() {
            self.gen_expr(r);
            emit!(self, "  mov %rax, %rdi");
            self.gen_expr(l); // simple: leaves %rdi alone
        } else {
            self.gen_expr(r);
            self.push();
            self.gen_expr(l);
            self.pop("%rdi");
        }
        None
    }

    fn cmp_zero(&mut self, ty: &Ty) {
        match &**ty {
            Type::Float => emit!(self, "  xorps %xmm1, %xmm1\n  ucomiss %xmm1, %xmm0"),
            Type::Double => emit!(self, "  xorpd %xmm1, %xmm1\n  ucomisd %xmm1, %xmm0"),
            Type::LDouble => emit!(self, "  fldz\n  fucomip %st(1), %st\n  fstp %st(0)"),
            _ if ty.is_integer() && ty.size() <= 4 => emit!(self, "  cmp $0, %eax"),
            _ => emit!(self, "  cmp $0, %rax"),
        }
        if ty.is_flonum() {
            // A NaN compares unordered but is not zero.
            emit!(self, "  setne %al\n  setp %dl\n  or %dl, %al\n  test %al, %al");
        }
    }

    /// Truncates %eax to a 1- or 2-byte integer, keeping it extended to 32 bits.
    fn trunc(&mut self, size: i64, unsigned: bool) {
        let x = if unsigned { 'z' } else { 's' };
        match size {
            1 => emit!(self, "  mov{}bl %al, %eax", x),
            2 => emit!(self, "  mov{}wl %ax, %eax", x),
            _ => {}
        }
    }

    fn cast(&mut self, from: &Ty, to: &Ty) {
        use Type::*;
        if let (LDouble, Void) = (&**from, &**to) {
            emit!(self, "  fstp %st(0)");
        }
        if matches!(**to, Void | Struct(_)) || matches!(**from, Struct(_) | Void) {
            return;
        }
        if let Bool = **to {
            self.cmp_zero(from);
            emit!(self, "  setne %al\n  movzbl %al, %eax");
            return;
        }
        match (&**from, &**to) {
            (Float, Float) | (Double, Double) | (LDouble, LDouble) => {}
            (Float, Double) => emit!(self, "  cvtss2sd %xmm0, %xmm0"),
            (Double, Float) => emit!(self, "  cvtsd2ss %xmm0, %xmm0"),
            // x87 conversions go through the red zone below %rsp.
            (Float, LDouble) => emit!(self, "  movss %xmm0, -4(%rsp)\n  flds -4(%rsp)"),
            (Double, LDouble) => emit!(self, "  movsd %xmm0, -8(%rsp)\n  fldl -8(%rsp)"),
            (LDouble, Float) => emit!(self, "  fstps -4(%rsp)\n  movss -4(%rsp), %xmm0"),
            (LDouble, Double) => emit!(self, "  fstpl -8(%rsp)\n  movsd -8(%rsp), %xmm0"),
            (LDouble, t) => {
                let (size, unsigned) = int_info(t);
                if size == 8 && unsigned {
                    // Values >= 2^63: subtract 2^63, convert, set the top bit.
                    emit!(self, "  movl $0x5f000000, -12(%rsp)\n  flds -12(%rsp)\n  fucomip %st(1), %st\n  ja 1f");
                    emit!(self, "  fsubs -12(%rsp)\n  fisttpq -8(%rsp)\n  mov -8(%rsp), %rax\n  btc $63, %rax\n  jmp 2f");
                    emit!(self, "1:\n  fisttpq -8(%rsp)\n  mov -8(%rsp), %rax\n2:");
                } else {
                    emit!(self, "  fisttpq -8(%rsp)\n  mov -8(%rsp), %rax");
                    self.trunc(size, unsigned);
                }
            }
            (f, LDouble) => {
                let (size, unsigned) = int_info(f);
                if size < 8 {
                    emit!(self, "  {}", if unsigned { "mov %eax, %eax" } else { "movslq %eax, %rax" });
                }
                emit!(self, "  mov %rax, -8(%rsp)\n  fildq -8(%rsp)");
                if size == 8 && unsigned {
                    // fildq is signed: add 2^64 back.
                    emit!(self, "  test %rax, %rax\n  jns 1f\n  movl $0x5f800000, -4(%rsp)\n  fadds -4(%rsp)\n1:");
                }
            }
            (Float | Double, t) => {
                let (size, unsigned) = int_info(t);
                if let Float = **from {
                    emit!(self, "  cvtss2sd %xmm0, %xmm0");
                }
                if size == 8 && unsigned {
                    // Values >= 2^63 do not fit a signed conversion.
                    emit!(self, "  mov $0x43e0000000000000, %rcx\n  movq %rcx, %xmm1\n  ucomisd %xmm1, %xmm0\n  jae 1f");
                    emit!(self, "  cvttsd2siq %xmm0, %rax\n  jmp 2f\n1:\n  subsd %xmm1, %xmm0\n  cvttsd2siq %xmm0, %rax\n  btc $63, %rax\n2:");
                } else if size == 8 || (size == 4 && unsigned) {
                    emit!(self, "  cvttsd2siq %xmm0, %rax");
                } else {
                    emit!(self, "  cvttsd2sil %xmm0, %eax");
                    self.trunc(size, unsigned);
                }
            }
            (f, Float | Double) => {
                let (size, unsigned) = int_info(f);
                let s = if let Float = **to { "ss" } else { "sd" };
                if size == 8 && unsigned {
                    emit!(self, "  test %rax, %rax\n  js 1f\n  pxor %xmm0, %xmm0\n  cvtsi2{}q %rax, %xmm0\n  jmp 2f", s);
                    emit!(self, "1:\n  mov %rax, %rdi\n  and $1, %eax\n  pxor %xmm0, %xmm0\n  shr %rdi\n  or %rax, %rdi");
                    emit!(self, "  cvtsi2{}q %rdi, %xmm0\n  add{} %xmm0, %xmm0\n2:", s, s);
                } else if size == 8 {
                    emit!(self, "  cvtsi2{}q %rax, %xmm0", s);
                } else if size == 4 && unsigned {
                    emit!(self, "  mov %eax, %eax\n  cvtsi2{}q %rax, %xmm0", s);
                } else {
                    emit!(self, "  cvtsi2{}l %eax, %xmm0", s);
                }
            }
            (f, t) => {
                let ((fs, fu), (ts, tu)) = (int_info(f), int_info(t));
                if ts < 4 {
                    self.trunc(ts, tu);
                } else if ts == 8 && fs < 8 {
                    if fs == 4 && fu {
                        emit!(self, "  mov %eax, %eax");
                    } else {
                        emit!(self, "  movslq %eax, %rax");
                    }
                }
            }
        }
    }

    fn gen_expr(&mut self, n: &Node) {
        match &n.kind {
            NK::Nop => {}
            NK::Num(v) => emit!(self, "  mov ${}, %rax", v),
            NK::FNum(v) => match &*n.ty {
                Type::Float => emit!(self, "  mov ${}, %eax\n  movd %eax, %xmm0", (*v as f32).to_bits()),
                _ => emit!(self, "  mov ${}, %rax\n  movq %rax, %xmm0", v.to_bits() as i64),
            },
            NK::LNum(m, se) => emit!(self, "  mov ${}, %rax\n  mov %rax, -16(%rsp)\n  movw ${}, -8(%rsp)\n  fldt -16(%rsp)", *m as i64, se),
            NK::Neg(l) => {
                self.gen_expr(l);
                match &*n.ty {
                    Type::LDouble => emit!(self, "  fchs"),
                    Type::Float => emit!(self, "  mov $1, %eax\n  shl $31, %eax\n  movd %eax, %xmm1\n  xorps %xmm1, %xmm0"),
                    Type::Double => emit!(self, "  mov $1, %eax\n  shl $63, %rax\n  movq %rax, %xmm1\n  xorpd %xmm1, %xmm0"),
                    _ => emit!(self, "  neg %rax"),
                }
            }
            NK::LVar(_) | NK::GVar(_) => match self.direct(n) {
                Some(m) => self.load_from(&n.ty, &m),
                None => {
                    self.gen_addr(n);
                    self.load(&n.ty);
                }
            },
            NK::Member(l, m) if m.bit_width < 0 => match self.direct(n) {
                Some(mem) => self.load_from(&n.ty, &mem),
                None => {
                    self.gen_addr(l);
                    self.load_from(&n.ty, &format!("{}(%rax)", m.offset));
                }
            },
            NK::Member(_, m) => {
                self.gen_addr(n);
                self.load(&n.ty);
                if m.bit_width >= 0 {
                    emit!(self, "  shl ${}, %rax", 64 - m.bit_width - m.bit_off);
                    emit!(self, "  {} ${}, %rax", if m.ty.is_unsigned() { "shr" } else { "sar" }, 64 - m.bit_width);
                }
            }
            NK::Deref(l) => {
                self.gen_expr(l);
                self.load(&n.ty);
            }
            NK::Addr(l) => self.gen_addr(l),
            NK::Assign(l, r) => {
                if let Some(m) = self.direct(l) {
                    // Evaluating the value first also keeps `x = setjmp(..)` working.
                    self.gen_expr(r);
                    self.store_to(&n.ty, &m);
                    return;
                }
                self.gen_addr(l);
                let bitfield = matches!(&l.kind, NK::Member(_, m) if m.bit_width >= 0);
                if !bitfield && (self.simple_load(r, "%rdi", "%edi").is_some() || self.simple_fload(r, 0).is_some()) {
                    emit!(self, "  mov %rax, %rdi");
                    self.gen_expr(r); // simple: leaves %rdi alone
                    self.store_to(&n.ty, "(%rdi)");
                    return;
                }
                self.push();
                self.gen_expr(r);
                if let NK::Member(_, m) = &l.kind {
                    if m.bit_width >= 0 {
                        // Merge the new value into the bitfield's storage unit.
                        let mask = (1u64 << m.bit_width) - 1;
                        emit!(self, "  mov ${}, %r9\n  and %r9, %rax\n  mov %rax, %r8\n  mov %rax, %rdi\n  shl ${}, %rdi", mask as i64, m.bit_off);
                        emit!(self, "  mov (%rsp), %rax");
                        self.load(&m.ty);
                        emit!(self, "  mov ${}, %r9\n  and %r9, %rax\n  or %rdi, %rax", !(mask << m.bit_off) as i64);
                        self.store(&n.ty);
                        // The value of the expression is the value stored in the bitfield.
                        emit!(self, "  mov %r8, %rax");
                        if !m.ty.is_unsigned() {
                            emit!(self, "  shl ${}, %rax\n  sar ${}, %rax", 64 - m.bit_width, 64 - m.bit_width);
                        }
                        return;
                    }
                }
                self.store(&n.ty);
            }
            NK::MemZero(i) => {
                let (size, off) = (self.func.unwrap().locals[*i].ty.size(), self.offs[*i]);
                if size <= 64 {
                    let mut k = 0;
                    for (step, ins) in [(8, "movq"), (4, "movl"), (2, "movw"), (1, "movb")] {
                        while size - k >= step {
                            emit!(self, "  {} $0, {}(%rbp)", ins, off + k);
                            k += step;
                        }
                    }
                } else {
                    emit!(self, "  mov ${}, %ecx\n  lea {}(%rbp), %rdi\n  mov $0, %al\n  rep stosb", size, off);
                }
            }
            NK::Alloca(size) => {
                self.gen_expr(size);
                self.alloca();
            }
            NK::VlaAlloc { var, size, save } => {
                // Reaching the declaration again frees the previous instance.
                let (v, s) = (self.offs[*var], self.offs[*save]);
                emit!(self, "  mov {}(%rbp), %rax\n  test %rax, %rax\n  je 1f\n  mov %rax, %rsp\n1:\n  mov %rsp, {}(%rbp)", s, s);
                self.gen_expr(size);
                self.alloca();
                emit!(self, "  mov %rax, {}(%rbp)", v);
            }
            NK::Cond(c, t, e) => {
                let id = self.next();
                self.gen_branch(c, &format!(".L.else.{}", id), false);
                self.gen_expr(t);
                emit!(self, "  jmp .L.end.{}\n.L.else.{}:", id, id);
                self.gen_expr(e);
                emit!(self, ".L.end.{}:", id);
            }
            NK::Not(l) => {
                self.gen_expr(l);
                self.cmp_zero(&l.ty);
                emit!(self, "  sete %al\n  movzbl %al, %eax");
            }
            NK::BitNot(l) => {
                self.gen_expr(l);
                emit!(self, "  not %rax");
            }
            NK::LogAnd(..) | NK::LogOr(..) => {
                let id = self.next();
                self.gen_branch(n, &format!(".L.false.{}", id), false);
                emit!(self, "  mov $1, %eax\n  jmp .L.end.{}\n.L.false.{}:\n  mov $0, %eax\n.L.end.{}:", id, id, id);
            }
            NK::Cast(l) => {
                self.gen_expr(l);
                self.cast(&l.ty, &n.ty);
            }
            NK::Comma(l, r) => {
                self.gen_void(l);
                self.gen_expr(r);
            }
            NK::StmtExpr(stmts) => {
                for (i, s) in stmts.iter().enumerate() {
                    match &s.kind {
                        // The last expression is the value: keep it.
                        NK::ExprStmt(e) if i + 1 == stmts.len() => self.gen_expr(e),
                        _ => self.gen_stmt(s),
                    }
                }
            }
            NK::VaStart(ap) => {
                let f = self.func.unwrap();
                let Some(area) = f.va_area else { error_at(n.loc.0, n.loc.1, "va_start used in a non-variadic function") };
                self.gen_expr(ap);
                emit!(self, "  movl ${}, (%rax)\n  movl ${}, 4(%rax)", self.va.0 * 8, 48 + self.va.1 * 16);
                emit!(self, "  lea {}(%rbp), %rdi\n  mov %rdi, 8(%rax)", self.va.2);
                emit!(self, "  lea {}(%rbp), %rdi\n  mov %rdi, 16(%rax)", self.offs[area]);
            }
            NK::LabelAddr(l) => emit!(self, "  lea {}(%rip), %rax", l),
            NK::FrameAddr => emit!(self, "  mov %rbp, %rax"),
            NK::Call { f, args, ret_buf } => self.gen_call(n, f, args, *ret_buf),
            NK::Bin(op, l, r) => self.gen_bin(*op, l, r),
            _ => error_at(n.loc.0, n.loc.1, "statement used as an expression"),
        }
    }

    /// Jumps to `label` if the truth value of `n` is `when`, and falls through
    /// otherwise. A constant condition gives an unconditional jump or nothing,
    /// which lets the dead-code pass drop the branch not taken.
    fn gen_branch(&mut self, n: &Node, label: &str, when: bool) {
        match &n.kind {
            NK::Num(v) => {
                if (*v != 0) == when {
                    emit!(self, "  jmp {}", label);
                }
            }
            NK::Not(l) => self.gen_branch(l, label, !when),
            NK::Comma(l, r) => {
                self.gen_void(l);
                self.gen_branch(r, label, when);
            }
            // `a && b` is false as soon as one operand is; `a || b` true likewise.
            NK::LogAnd(l, r) | NK::LogOr(l, r) => {
                let is_and = matches!(n.kind, NK::LogAnd(..));
                if is_and != when {
                    self.gen_branch(l, label, when);
                    self.gen_branch(r, label, when);
                } else {
                    let skip = format!(".L.skip.{}", self.next());
                    self.gen_branch(l, &skip, !when);
                    self.gen_branch(r, label, when);
                    emit!(self, "{}:", skip);
                }
            }
            NK::Bin(op @ (Op::Eq | Op::Ne | Op::Lt | Op::Le), l, r) if !l.ty.is_flonum() => {
                let wide = l.ty.size() == 8 || l.ty.base().is_some();
                match self.gen_operands(*op, l, r) {
                    Some(v) => emit!(self, "  cmp ${}, {}", v, if wide { "%rax" } else { "%eax" }),
                    None => emit!(self, "  cmp {}", if wide { "%rdi, %rax" } else { "%edi, %eax" }),
                }
                let cc = match (op, l.ty.is_unsigned(), when) {
                    (Op::Eq, _, true) | (Op::Ne, _, false) => "e",
                    (Op::Eq, _, false) | (Op::Ne, _, true) => "ne",
                    (Op::Lt, false, true) => "l",
                    (Op::Lt, false, false) => "ge",
                    (Op::Lt, true, true) => "b",
                    (Op::Lt, true, false) => "ae",
                    (_, false, true) => "le",
                    (_, false, false) => "g",
                    (_, true, true) => "be",
                    (_, true, false) => "a",
                };
                emit!(self, "  j{} {}", cc, label);
            }
            _ => {
                self.gen_expr(n);
                self.cmp_zero(&n.ty);
                emit!(self, "  {} {}", if when { "jne" } else { "je" }, label);
            }
        }
    }

    /// Allocates %rax bytes on the stack below the frame; the temporaries
    /// currently pushed (`depth` words) are moved down to stay on top.
    fn alloca(&mut self) {
        emit!(self, "  add $15, %rax\n  and $-16, %rax\n  mov %rsp, %rsi\n  sub %rax, %rsp\n  mov %rsp, %rdi");
        emit!(self, "  mov ${}, %ecx\n  rep movsq\n  mov %rdi, %rax", self.depth);
    }

    fn gen_bin(&mut self, op: Op, l: &Node, r: &Node) {
        if l.ty.is_flonum() {
            // Long double: %st(0) = right operand, %st(1) = left operand.
            let ld = matches!(*l.ty, Type::LDouble);
            if ld {
                self.gen_expr(r);
                self.pushld();
                self.gen_expr(l);
                self.popld();
            } else if let Some(ins) = self.simple_fload(r, 1) {
                self.gen_expr(l);
                emit!(self, "{}", ins);
            } else if self.simple_fload(l, 0).is_some() {
                self.gen_expr(r);
                emit!(self, "  movaps %xmm0, %xmm1");
                self.gen_expr(l); // simple: leaves %xmm1 alone
            } else {
                self.gen_expr(r);
                self.pushf();
                self.gen_expr(l);
                self.popf(1);
            }
            let s = if let Type::Float = *l.ty { "ss" } else { "sd" };
            match op {
                Op::Add if ld => emit!(self, "  faddp"),
                Op::Sub if ld => emit!(self, "  fsubrp"),
                Op::Mul if ld => emit!(self, "  fmulp"),
                Op::Div if ld => emit!(self, "  fdivrp"),
                Op::Add => emit!(self, "  add{} %xmm1, %xmm0", s),
                Op::Sub => emit!(self, "  sub{} %xmm1, %xmm0", s),
                Op::Mul => emit!(self, "  mul{} %xmm1, %xmm0", s),
                Op::Div => emit!(self, "  div{} %xmm1, %xmm0", s),
                Op::Eq | Op::Ne | Op::Lt | Op::Le => {
                    if ld {
                        emit!(self, "  fucomip %st(1), %st\n  fstp %st(0)");
                    } else {
                        emit!(self, "  ucomi{} %xmm0, %xmm1", s);
                    }
                    match op {
                        Op::Eq => emit!(self, "  sete %al\n  setnp %dl\n  and %dl, %al"),
                        Op::Ne => emit!(self, "  setne %al\n  setp %dl\n  or %dl, %al"),
                        Op::Lt => emit!(self, "  seta %al"),
                        _ => emit!(self, "  setae %al"),
                    }
                    emit!(self, "  movzbl %al, %eax");
                }
                _ => error_at(l.loc.0, l.loc.1, "invalid operands"),
            }
            return;
        }
        let imm = self.gen_operands(op, l, r);
        let wide = l.ty.size() == 8 || l.ty.base().is_some();
        let (ax, di, dx) = if wide { ("%rax", "%rdi", "%rdx") } else { ("%eax", "%edi", "%edx") };
        let imm_text = imm.map(|v| format!("${}", v));
        let di = imm_text.as_deref().unwrap_or(di);
        let uns = l.ty.is_unsigned();
        match op {
            Op::Add => emit!(self, "  add {}, {}", di, ax),
            Op::Sub => emit!(self, "  sub {}, {}", di, ax),
            Op::Mul if imm.is_some() => emit!(self, "  imul {}, {}, {}", di, ax, ax),
            Op::Shl | Op::Shr if imm.is_some() => {
                let ins = if op == Op::Shl { "shl" } else if uns { "shr" } else { "sar" };
                emit!(self, "  {} ${}, {}", ins, imm.unwrap() & 63, ax);
            }
            Op::Mul => emit!(self, "  imul {}, {}", di, ax),
            Op::Div | Op::Mod => {
                if uns {
                    emit!(self, "  mov $0, {}\n  div {}", dx, di);
                } else {
                    emit!(self, "  {}\n  idiv {}", if wide { "cqo" } else { "cdq" }, di);
                }
                if op == Op::Mod {
                    emit!(self, "  mov %rdx, %rax");
                }
            }
            Op::BitAnd => emit!(self, "  and {}, {}", di, ax),
            Op::BitOr => emit!(self, "  or {}, {}", di, ax),
            Op::BitXor => emit!(self, "  xor {}, {}", di, ax),
            Op::Eq | Op::Ne | Op::Lt | Op::Le => {
                let cc = match (op, uns) {
                    (Op::Eq, _) => "e",
                    (Op::Ne, _) => "ne",
                    (Op::Lt, false) => "l",
                    (Op::Lt, true) => "b",
                    (_, false) => "le",
                    (_, true) => "be",
                };
                emit!(self, "  cmp {}, {}\n  set{} %al\n  movzbl %al, %eax", di, ax, cc);
            }
            Op::Shl => emit!(self, "  mov %edi, %ecx\n  shl %cl, {}", ax),
            Op::Shr => emit!(self, "  mov %edi, %ecx\n  {} %cl, {}", if uns { "shr" } else { "sar" }, ax),
        }
    }

    fn push_arg(&mut self, a: &Node) {
        self.gen_expr(a);
        match &*a.ty {
            Type::Struct(_) => {
                let sz = align_to(a.ty.size(), 8);
                emit!(self, "  sub ${}, %rsp", sz);
                self.depth += sz / 8;
                emit!(self, "  mov %rax, %rsi\n  mov %rsp, %rdi\n  mov ${}, %ecx\n  rep movsb", a.ty.size());
            }
            Type::Float | Type::Double => self.pushf(),
            Type::LDouble => self.pushld(),
            _ => self.push(),
        }
    }

    /// Size of an argument passed on the stack.
    fn stack_size(ty: &Ty) -> i64 {
        match &**ty {
            Type::Struct(_) | Type::LDouble => align_to(ty.size(), 8),
            _ => 8,
        }
    }

    fn gen_call(&mut self, n: &Node, f: &Node, args: &[Node], ret_buf: Option<usize>) {
        let ret = &n.ty;
        let ret_mem = mem_class(ret);
        let (mut gp, mut fp) = (ret_mem as usize, 0);
        let on_stack: Vec<bool> = args.iter().map(|a| !in_regs(&a.ty, &mut gp, &mut fp)).collect();
        // Offsets of the stack arguments from %rsp at the call; long doubles and
        // structs containing them are 16-byte aligned.
        let (mut arg_offs, mut total) = (vec![0i64; args.len()], 0i64);
        for (i, a) in args.iter().enumerate().filter(|(i, _)| on_stack[*i]) {
            if matches!(&*a.ty, Type::Struct(_) | Type::LDouble) && a.ty.align() >= 16 {
                total = align_to(total, 16);
            }
            arg_offs[i] = total;
            total += Self::stack_size(&a.ty);
        }
        let mut slots = total / 8;
        if (self.depth + slots) % 2 == 1 {
            emit!(self, "  sub $8, %rsp");
            self.depth += 1;
            slots += 1;
        }
        // Stack arguments first (in reverse, with padding), then register arguments.
        let mut end = total;
        for (i, a) in args.iter().enumerate().rev().filter(|(i, _)| on_stack[*i]) {
            let pad = end - arg_offs[i] - Self::stack_size(&a.ty);
            if pad > 0 {
                emit!(self, "  sub ${}, %rsp", pad);
                self.depth += pad / 8;
            }
            self.push_arg(a);
            end = arg_offs[i];
        }
        // Simple register arguments are not pushed: they are loaded straight into
        // their registers once the others have been evaluated.
        let mut late: Vec<Option<String>> = vec![None; args.len()];
        let (mut ng, mut nf) = (ret_mem as usize, 0);
        for (i, a) in args.iter().enumerate().filter(|(i, _)| !on_stack[*i]) {
            match &*a.ty {
                Type::Struct(_) => {
                    for lo in [0, 8] {
                        if lo < a.ty.size() {
                            if has_flonum(&a.ty, lo, lo + 8, 0) { nf += 1 } else { ng += 1 }
                        }
                    }
                }
                Type::Float | Type::Double => {
                    late[i] = self.simple_fload(a, nf);
                    nf += 1;
                }
                _ => {
                    late[i] = self.simple_load(a, ARG64[ng], ARG32[ng]);
                    ng += 1;
                }
            }
        }
        for (_, a) in args.iter().enumerate().rev().filter(|(i, _)| !on_stack[*i] && late[*i].is_none()) {
            self.push_arg(a);
        }
        let direct = match &f.kind {
            NK::GVar(name) if matches!(*f.ty, Type::Func(_)) => Some(name),
            _ => None,
        };
        if direct.is_none() {
            self.gen_expr(f);
            emit!(self, "  mov %rax, %r10");
        }
        let (mut gp, mut fp) = (0, 0);
        if ret_mem {
            emit!(self, "  lea {}(%rbp), %rdi", self.offs[ret_buf.unwrap()]);
            gp = 1;
        }
        for (i, a) in args.iter().enumerate().filter(|(i, _)| !on_stack[*i]) {
            match &*a.ty {
                Type::Struct(_) => {
                    for lo in [0, 8] {
                        if lo >= a.ty.size() {
                            continue;
                        }
                        if has_flonum(&a.ty, lo, lo + 8, 0) {
                            self.popf(fp);
                            fp += 1;
                        } else {
                            self.pop(ARG64[gp]);
                            gp += 1;
                        }
                    }
                }
                Type::Float | Type::Double => {
                    if late[i].is_none() {
                        self.popf(fp);
                    }
                    fp += 1;
                }
                _ => {
                    if late[i].is_none() {
                        self.pop(ARG64[gp]);
                    }
                    gp += 1;
                }
            }
        }
        for ins in late.iter().flatten() {
            emit!(self, "{}", ins);
        }
        emit!(self, "  mov ${}, %eax", fp);
        match direct {
            Some(name) => emit!(self, "  call {}@PLT", name),
            None => emit!(self, "  call *%r10"),
        }
        if slots > 0 {
            emit!(self, "  add ${}, %rsp", slots * 8);
            self.depth -= slots;
        }
        match &**ret {
            Type::Bool => emit!(self, "  movzbl %al, %eax"),
            Type::Int { size, unsigned } => self.trunc(*size as i64, *unsigned),
            Type::Struct(_) if !ret_mem && has_ldouble(ret) => {
                // struct { long double } comes back in %st(0).
                let off = self.offs[ret_buf.unwrap()];
                emit!(self, "  fstpt {}(%rbp)\n  lea {}(%rbp), %rax", off, off);
            }
            Type::Struct(_) if !ret_mem => {
                // Small structs come back in rax/rdx/xmm0/xmm1.
                let (off, size) = (self.offs[ret_buf.unwrap()], ret.size());
                let (mut gp, mut fp) = (0, 0);
                for lo in [0, 8] {
                    let n = (size - lo).min(8);
                    if n <= 0 {
                        continue;
                    }
                    if has_flonum(ret, lo, lo + 8, 0) {
                        emit!(self, "  {} %xmm{}, {}(%rbp)", if n == 4 { "movss" } else { "movsd" }, fp, off + lo);
                        fp += 1;
                    } else {
                        let (r8, r64) = if gp == 0 { ("%al", "%rax") } else { ("%dl", "%rdx") };
                        for i in 0..n {
                            emit!(self, "  mov {}, {}(%rbp)\n  shr $8, {}", r8, off + lo + i, r64);
                        }
                        gp += 1;
                    }
                }
                emit!(self, "  lea {}(%rbp), %rax", off);
            }
            _ => {}
        }
    }

    // ----- statements -----

    fn gen_stmt(&mut self, n: &Node) {
        match &n.kind {
            NK::Nop => {}
            NK::Return(e) => {
                let f = self.func.unwrap();
                if let Some(e) = e {
                    self.gen_expr(e);
                    if e.ty.is_struct() {
                        let size = e.ty.size();
                        if mem_class(&e.ty) {
                            // Copy to the caller's buffer and return its address.
                            let off = self.offs[f.params[0]];
                            emit!(self, "  mov %rax, %rsi\n  mov {}(%rbp), %rdi\n  mov ${}, %ecx\n  rep movsb\n  mov {}(%rbp), %rax", off, size, off);
                        } else if has_ldouble(&e.ty) {
                            emit!(self, "  fldt (%rax)");
                        } else {
                            emit!(self, "  mov %rax, %rdi");
                            let (mut gp, mut fp) = (0, 0);
                            for lo in [0, 8] {
                                let cnt = (size - lo).min(8);
                                if cnt <= 0 {
                                    continue;
                                }
                                if has_flonum(&e.ty, lo, lo + 8, 0) {
                                    emit!(self, "  {} {}(%rdi), %xmm{}", if cnt == 4 { "movss" } else { "movsd" }, lo, fp);
                                    fp += 1;
                                } else {
                                    let (r8, r64) = if gp == 0 { ("%al", "%rax") } else { ("%dl", "%rdx") };
                                    emit!(self, "  mov $0, {}", r64);
                                    for i in (0..cnt).rev() {
                                        emit!(self, "  shl $8, {}\n  mov {}(%rdi), {}", r64, lo + i, r8);
                                    }
                                    gp += 1;
                                }
                            }
                        }
                    }
                }
                emit!(self, "  jmp .L.return.{}", f.name);
            }
            NK::If(c, t, e) => {
                let id = self.next();
                self.gen_branch(c, &format!(".L.else.{}", id), false);
                self.gen_stmt(t);
                emit!(self, "  jmp .L.end.{}\n.L.else.{}:", id, id);
                if let Some(e) = e {
                    self.gen_stmt(e);
                }
                emit!(self, ".L.end.{}:", id);
            }
            NK::For { init, cond, inc, body, brk, cont } => {
                let id = self.next();
                if let Some(i) = init {
                    self.gen_stmt(i);
                }
                emit!(self, ".L.begin.{}:", id);
                if let Some(c) = cond {
                    self.gen_branch(c, brk, false);
                }
                self.gen_stmt(body);
                emit!(self, "{}:", cont);
                if let Some(i) = inc {
                    self.gen_void(i);
                }
                emit!(self, "  jmp .L.begin.{}\n{}:", id, brk);
            }
            NK::Do { body, cond, brk, cont } => {
                let id = self.next();
                emit!(self, ".L.begin.{}:", id);
                self.gen_stmt(body);
                emit!(self, "{}:", cont);
                self.gen_branch(cond, &format!(".L.begin.{}", id), true);
                emit!(self, "{}:", brk);
            }
            NK::Switch { cond, body, cases, default, brk } => {
                self.gen_expr(cond);
                let wide = cond.ty.size() == 8;
                for (v, hi, l) in cases {
                    if v != hi {
                        // Range: (x - lo) <= (hi - lo), unsigned.
                        if wide {
                            emit!(self, "  mov %rax, %rdi\n  mov ${}, %rsi\n  sub %rsi, %rdi\n  mov ${}, %rsi\n  cmp %rsi, %rdi", v, hi.wrapping_sub(*v));
                        } else {
                            emit!(self, "  mov %eax, %edi\n  sub ${}, %edi\n  cmp ${}, %edi", *v as i32, hi.wrapping_sub(*v) as i32);
                        }
                        emit!(self, "  jbe {}", l);
                        continue;
                    }
                    if !wide {
                        emit!(self, "  cmp ${}, %eax", *v as i32);
                    } else if *v == *v as i32 as i64 {
                        emit!(self, "  cmp ${}, %rax", v);
                    } else {
                        emit!(self, "  mov ${}, %rdi\n  cmp %rdi, %rax", v);
                    }
                    emit!(self, "  je {}", l);
                }
                emit!(self, "  jmp {}", default.as_ref().unwrap_or(brk));
                self.gen_stmt(body);
                emit!(self, "{}:", brk);
            }
            NK::Label(l, body) => {
                emit!(self, "{}:", l);
                self.gen_stmt(body);
            }
            NK::Goto(l) => emit!(self, "  jmp {}", l),
            NK::Asm(text) => {
                self.has_asm = true;
                emit!(self, "  {}", text);
            }
            NK::GotoExpr(e) => {
                self.gen_expr(e);
                emit!(self, "  jmp *%rax");
            }
            NK::Block(v) => {
                for s in v {
                    self.gen_stmt(s);
                }
            }
            NK::ExprStmt(e) => self.gen_void(e),
            _ => error_at(n.loc.0, n.loc.1, "invalid statement"),
        }
    }
}
