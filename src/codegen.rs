// x86-64 System V code generator. A simple stack machine in the style of
// chibicc: every expression leaves its value in %rax (or %xmm0 for floats).
use crate::lex::error_at;
use crate::parse::{Node, Obj, Op, NK};
use crate::types::*;
use std::collections::HashSet;
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
        _ => off < lo || hi <= off,
    }
}

/// Decides whether an argument of the given type is passed in registers,
/// updating the counts of used general-purpose and SSE registers.
fn in_regs(ty: &Ty, gp: &mut usize, fp: &mut usize) -> bool {
    let (ng, nf) = match &**ty {
        Type::Struct(_) => {
            if ty.size() > 16 {
                return false;
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
    func: Option<&'a Obj>,
    offs: Vec<i64>,
    va: (usize, usize, i64), // gp regs used, fp regs used, start of stack args
}

pub fn codegen(globals: &[Obj]) -> String {
    let mut g = Gen { out: String::new(), depth: 0, cnt: 0, defined: HashSet::new(), func: None, offs: vec![], va: (0, 0, 0) };
    g.defined = globals.iter().filter(|o| o.is_def).map(|o| o.name.as_str()).collect();

    // Static functions are emitted only if reachable from something emitted.
    let mut live: HashSet<&str> = HashSet::new();
    let mut work: Vec<&str> = Vec::new();
    for o in globals.iter().filter(|o| o.is_def) {
        if !o.is_func {
            work.extend(o.relocs.iter().map(|r| r.label.as_str()));
        } else if !o.is_static {
            work.push(&o.name);
        }
    }
    while let Some(name) = work.pop() {
        if !live.insert(name) {
            continue;
        }
        if let Some(o) = globals.iter().find(|o| o.name == name && o.is_func) {
            work.extend(o.refs.iter().map(|s| s.as_str()));
        }
    }

    for o in globals.iter().filter(|o| o.is_def && !o.is_func) {
        g.emit_data(o);
    }
    for o in globals.iter().filter(|o| o.is_def && o.is_func && live.contains(o.name.as_str())) {
        g.emit_func(o);
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

    fn emit_data(&mut self, o: &Obj) {
        let size = o.ty.size();
        let align = if matches!(&*o.ty, Type::Array(..)) && size >= 16 { o.ty.align().max(16) } else { o.ty.align() };
        if !o.name.starts_with(".L") {
            emit!(self, "  .{} {}", if o.is_static { "local" } else { "globl" }, o.name);
        }
        let Some(data) = &o.init else {
            emit!(self, "  .bss\n  .balign {}\n{}:\n  .zero {}", align, o.name, size);
            return;
        };
        emit!(self, "  .data\n  .balign {}\n{}:", align, o.name);
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

        // Assign stack offsets to parameters and locals.
        self.offs = vec![0; f.locals.len()];
        let (mut gp, mut fp, mut top) = (0, 0, 16i64);
        let mut on_stack = vec![false; f.locals.len()];
        for &p in &f.params {
            let ty = &f.locals[p].ty;
            if !in_regs(ty, &mut gp, &mut fp) {
                on_stack[p] = true;
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
            bottom = align_to(bottom + size, align.max(1));
            self.offs[i] = -bottom;
        }

        emit!(self, "  .{} {}", if f.is_static { "local" } else { "globl" }, f.name);
        emit!(self, "  .text\n  .type {}, @function\n{}:", f.name, f.name);
        emit!(self, "  push %rbp\n  mov %rsp, %rbp\n  sub ${}, %rsp", align_to(bottom, 16));

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
            NK::LVar(i) => emit!(self, "  lea {}(%rbp), %rax", self.offs[*i]),
            NK::GVar(name) => {
                if self.defined.contains(name.as_str()) {
                    emit!(self, "  lea {}(%rip), %rax", name);
                } else {
                    emit!(self, "  mov {}@GOTPCREL(%rip), %rax", name);
                }
            }
            NK::Deref(l) => self.gen_expr(l),
            NK::Comma(l, r) => {
                self.gen_expr(l);
                self.gen_addr(r);
            }
            NK::Member(l, m) => {
                self.gen_addr(l);
                emit!(self, "  add ${}, %rax", m.offset);
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
            Type::Array(..) | Type::Struct(_) | Type::Func(_) | Type::Void => {
                if src != "(%rax)" {
                    emit!(self, "  lea {}, %rax", src);
                }
            }
            Type::Float => emit!(self, "  movss {}, %xmm0", src),
            Type::Double => emit!(self, "  movsd {}, %xmm0", src),
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
                emit!(self, "  mov %rax, %rsi\n  mov ${}, %ecx\n  rep movsb", ty.size());
            }
            Type::Float => emit!(self, "  movss %xmm0, {}", dst),
            Type::Double => emit!(self, "  movsd %xmm0, {}", dst),
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
            NK::LVar(i) => Some(format!("{}(%rbp)", self.offs[*i])),
            NK::GVar(name) if self.defined.contains(name.as_str()) => Some(format!("{}(%rip)", name)),
            _ => None,
        }
    }

    fn cmp_zero(&mut self, ty: &Ty) {
        match &**ty {
            Type::Float => emit!(self, "  xorps %xmm1, %xmm1\n  ucomiss %xmm1, %xmm0"),
            Type::Double => emit!(self, "  xorpd %xmm1, %xmm1\n  ucomisd %xmm1, %xmm0"),
            _ if ty.is_integer() && ty.size() <= 4 => emit!(self, "  cmp $0, %eax"),
            _ => emit!(self, "  cmp $0, %rax"),
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
        if matches!(**to, Void | Struct(_)) || matches!(**from, Struct(_) | Void) {
            return;
        }
        if let Bool = **to {
            self.cmp_zero(from);
            emit!(self, "  setne %al\n  movzbl %al, %eax");
            return;
        }
        match (&**from, &**to) {
            (Float, Float) | (Double, Double) => {}
            (Float, Double) => emit!(self, "  cvtss2sd %xmm0, %xmm0"),
            (Double, Float) => emit!(self, "  cvtsd2ss %xmm0, %xmm0"),
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
            NK::Neg(l) => {
                self.gen_expr(l);
                match &*n.ty {
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
                self.push();
                self.gen_expr(r);
                if let NK::Member(_, m) = &l.kind {
                    if m.bit_width >= 0 {
                        // Merge the new value into the bitfield's storage unit.
                        let mask = (1u64 << m.bit_width) - 1;
                        emit!(self, "  mov %rax, %r8\n  mov ${}, %r9\n  and %r9, %rax\n  mov %rax, %rdi\n  shl ${}, %rdi", mask as i64, m.bit_off);
                        emit!(self, "  mov (%rsp), %rax");
                        self.load(&m.ty);
                        emit!(self, "  mov ${}, %r9\n  and %r9, %rax\n  or %rdi, %rax", !(mask << m.bit_off) as i64);
                        self.store(&n.ty);
                        emit!(self, "  mov %r8, %rax");
                        return;
                    }
                }
                self.store(&n.ty);
            }
            NK::MemZero(i) => {
                let f = self.func.unwrap();
                emit!(self, "  mov ${}, %ecx\n  lea {}(%rbp), %rdi\n  mov $0, %al\n  rep stosb", f.locals[*i].ty.size(), self.offs[*i]);
            }
            NK::Cond(c, t, e) => {
                let id = self.next();
                self.gen_expr(c);
                self.cmp_zero(&c.ty);
                emit!(self, "  je .L.else.{}", id);
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
            NK::LogAnd(l, r) | NK::LogOr(l, r) => {
                let id = self.next();
                let (jmp, short) = if let NK::LogAnd(..) = n.kind { ("je", 0) } else { ("jne", 1) };
                self.gen_expr(l);
                self.cmp_zero(&l.ty);
                emit!(self, "  {} .L.short.{}", jmp, id);
                self.gen_expr(r);
                self.cmp_zero(&r.ty);
                emit!(self, "  {} .L.short.{}", jmp, id);
                emit!(self, "  mov ${}, %eax\n  jmp .L.end.{}\n.L.short.{}:\n  mov ${}, %eax\n.L.end.{}:", 1 - short, id, id, short, id);
            }
            NK::Cast(l) => {
                self.gen_expr(l);
                self.cast(&l.ty, &n.ty);
            }
            NK::Comma(l, r) => {
                self.gen_expr(l);
                self.gen_expr(r);
            }
            NK::StmtExpr(stmts) => {
                for s in stmts {
                    self.gen_stmt(s);
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
            NK::Call { f, args, ret_buf } => self.gen_call(n, f, args, *ret_buf),
            NK::Bin(op, l, r) => self.gen_bin(*op, l, r),
            _ => error_at(n.loc.0, n.loc.1, "statement used as an expression"),
        }
    }

    fn gen_bin(&mut self, op: Op, l: &Node, r: &Node) {
        if l.ty.is_flonum() {
            self.gen_expr(r);
            self.pushf();
            self.gen_expr(l);
            self.popf(1);
            let s = if let Type::Float = *l.ty { "ss" } else { "sd" };
            match op {
                Op::Add => emit!(self, "  add{} %xmm1, %xmm0", s),
                Op::Sub => emit!(self, "  sub{} %xmm1, %xmm0", s),
                Op::Mul => emit!(self, "  mul{} %xmm1, %xmm0", s),
                Op::Div => emit!(self, "  div{} %xmm1, %xmm0", s),
                Op::Eq | Op::Ne | Op::Lt | Op::Le => {
                    emit!(self, "  ucomi{} %xmm0, %xmm1", s);
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
        self.gen_expr(r);
        self.push();
        self.gen_expr(l);
        self.pop("%rdi");
        let wide = l.ty.size() == 8 || l.ty.base().is_some();
        let (ax, di, dx) = if wide { ("%rax", "%rdi", "%rdx") } else { ("%eax", "%edi", "%edx") };
        let uns = l.ty.is_unsigned();
        match op {
            Op::Add => emit!(self, "  add {}, {}", di, ax),
            Op::Sub => emit!(self, "  sub {}, {}", di, ax),
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
            _ => self.push(),
        }
    }

    fn gen_call(&mut self, n: &Node, f: &Node, args: &[Node], ret_buf: Option<usize>) {
        let ret = &n.ty;
        let ret_mem = ret.is_struct() && ret.size() > 16;
        let (mut gp, mut fp) = (ret_mem as usize, 0);
        let on_stack: Vec<bool> = args.iter().map(|a| !in_regs(&a.ty, &mut gp, &mut fp)).collect();
        let mut slots: i64 = args.iter().zip(&on_stack).filter(|(_, s)| **s).map(|(a, _)| if a.ty.is_struct() { align_to(a.ty.size(), 8) / 8 } else { 1 }).sum();
        if (self.depth + slots) % 2 == 1 {
            emit!(self, "  sub $8, %rsp");
            self.depth += 1;
            slots += 1;
        }
        // Stack arguments first (in reverse), then register arguments.
        for pass in [true, false] {
            for (a, _) in args.iter().zip(&on_stack).rev().filter(|(_, s)| **s == pass) {
                self.push_arg(a);
            }
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
        for (a, _) in args.iter().zip(&on_stack).filter(|(_, s)| !**s) {
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
                    self.popf(fp);
                    fp += 1;
                }
                _ => {
                    self.pop(ARG64[gp]);
                    gp += 1;
                }
            }
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
                        if size > 16 {
                            // Copy to the caller's buffer and return its address.
                            let off = self.offs[f.params[0]];
                            emit!(self, "  mov %rax, %rsi\n  mov {}(%rbp), %rdi\n  mov ${}, %ecx\n  rep movsb\n  mov {}(%rbp), %rax", off, size, off);
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
                self.gen_expr(c);
                self.cmp_zero(&c.ty);
                emit!(self, "  je .L.else.{}", id);
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
                    self.gen_expr(c);
                    self.cmp_zero(&c.ty);
                    emit!(self, "  je {}", brk);
                }
                self.gen_stmt(body);
                emit!(self, "{}:", cont);
                if let Some(i) = inc {
                    self.gen_expr(i);
                }
                emit!(self, "  jmp .L.begin.{}\n{}:", id, brk);
            }
            NK::Do { body, cond, brk, cont } => {
                let id = self.next();
                emit!(self, ".L.begin.{}:", id);
                self.gen_stmt(body);
                emit!(self, "{}:", cont);
                self.gen_expr(cond);
                self.cmp_zero(&cond.ty);
                emit!(self, "  jne .L.begin.{}\n{}:", id, brk);
            }
            NK::Switch { cond, body, cases, default, brk } => {
                self.gen_expr(cond);
                let wide = cond.ty.size() == 8;
                for (v, l) in cases {
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
            NK::Block(v) => {
                for s in v {
                    self.gen_stmt(s);
                }
            }
            NK::ExprStmt(e) => self.gen_expr(e),
            _ => error_at(n.loc.0, n.loc.1, "invalid statement"),
        }
    }
}
