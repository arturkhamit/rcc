// Register allocation and x86-64 emission for the IR of ir.rs.
//
// Registers. General-purpose ones are numbered as in codegen::GP. %rax, %rcx
// and %rdx are never allocated: the emitter uses them for values that live in
// the frame (%rax a value, %rcx and %rdx the base and index of an address) and
// for instructions with fixed registers (division, shifts). Likewise %xmm0,
// %xmm1 and (for parallel moves) %xmm15.
use crate::codegen::{reg_name, GP};
use crate::ir::*;
use crate::parse::Op;
use std::fmt::Write;

const AX: u8 = 0;
const CX: u8 = 1;
const DX: u8 = 2;
const SI: u8 = 3;
const DI: u8 = 4;
const R8: u8 = 5;
const R9: u8 = 6;
const R10: u8 = 7;
const R11: u8 = 8;
const ARGS: [u8; 6] = [DI, SI, DX, CX, R8, R9];
/// Allocatable general registers: the caller-saved ones first (they cost no
/// save and restore), then the callee-saved %rbx and %r12-%r15.
const INT_POOL: [u8; 11] = [SI, DI, R8, R9, R10, R11, 9, 10, 11, 12, 13];
const CALLER_SAVED: u32 = 1 << SI | 1 << DI | 1 << R8 | 1 << R9 | 1 << R10 | 1 << R11;
const FLT_POOL: std::ops::RangeInclusive<u8> = 2..=14;

#[derive(Clone, Copy, PartialEq)]
enum Loc {
    Reg(u8),   // a general register, or %xmmN for a float
    Slot(i64), // 8 bytes at this offset from %rbp
}

macro_rules! emit {
    ($s:expr, $($arg:tt)*) => { writeln!($s.out, $($arg)*).unwrap() };
}

/// Linear-scan register allocation (Poletto and Sarkar): every virtual
/// register gets one location for its whole life, a register or a frame slot.
/// Returns the locations, the frame size including the slots, and the first
/// and last position of every register's life.
fn allocate(f: &Func) -> (Vec<Loc>, i64, Vec<usize>, Vec<usize>) {
    let (n, nb) = (f.cls.len(), f.blocks.len());
    // Linear positions of the instructions, in layout order.
    let (mut first, mut last, mut npos) = (vec![0; nb], vec![0; nb], 0);
    for &b in &f.layout {
        first[b] = npos;
        npos += f.blocks[b].len();
        last[b] = npos - 1;
    }

    let (live_in, live_out) = liveness(f);

    // How often a block runs, as a guess: ten times more for every loop around
    // it. A loop is what lies between a block and a later one that jumps back.
    let mut index = vec![0; nb];
    f.layout.iter().enumerate().for_each(|(i, &b)| index[b] = i);
    let mut loop_end = vec![0; f.layout.len()]; // by the loop's first block: its last one
    for (i, &b) in f.layout.iter().enumerate() {
        for s in f.blocks[b].last().unwrap().succs() {
            if index[s] <= i {
                loop_end[index[s]] = loop_end[index[s]].max(i + 1);
            }
        }
    }
    let (mut freq, mut ends) = (vec![1u64; nb], vec![]);
    for (i, &b) in f.layout.iter().enumerate() {
        ends.retain(|e| *e > i);
        if loop_end[i] > 0 {
            ends.push(loop_end[i]);
        }
        freq[b] = 10u64.pow(ends.len().min(6) as u32);
    }

    // One interval per register: from its first to its last live position; and
    // its weight, the (guessed) number of times it is read or written.
    let mut weight = vec![0u64; n];
    let (mut start, mut end) = (vec![usize::MAX; n], vec![0usize; n]);
    let mut touch = |v: V, p: usize| {
        start[v] = start[v].min(p);
        end[v] = end[v].max(p);
    };
    // Positions that clobber registers: calls (all caller-saved ones) and the
    // string instructions of big copies (%rsi, %rdi). As prefix sums.
    let (mut calls, mut strs) = (vec![0u32; npos + 1], vec![0u32; npos + 1]);
    for &b in &f.layout {
        bits(&live_in[b]).for_each(|v| touch(v, first[b]));
        bits(&live_out[b]).for_each(|v| touch(v, last[b]));
        for (i, ins) in f.blocks[b].iter().enumerate() {
            let p = first[b] + i;
            ins.regs(&mut |v, _| {
                touch(v, p);
                weight[v] += freq[b];
            });
            calls[p + 1] = calls[p] + matches!(ins, Ins::Call(..)) as u32;
            strs[p + 1] = strs[p] + matches!(ins, Ins::Copy(_, _, size) | Ins::Zero(_, size) if *size > 64) as u32;
        }
    }
    // Whether a clobbering position lies strictly inside the interval of `v`.
    let crosses = |v: V, sums: &[u32]| end[v] > start[v] + 1 && sums[end[v]] > sums[start[v] + 1];

    // A parameter stays in the register it arrives in, and an argument is
    // computed in the register it is passed in, where that is possible.
    let mut hint: Vec<Option<u8>> = vec![None; n];
    for &b in &f.layout {
        for ins in &f.blocks[b] {
            match ins {
                Ins::Entry(params) => {
                    for &(v, i) in params {
                        hint[v] = Some(if f.cls[v] == Cls::Int { ARGS[i] } else { i as u8 });
                    }
                }
                Ins::Call(_, _, _, args, _) => {
                    let (mut ints, mut flts) = (0, 0);
                    for arg in args {
                        if let Arg::Reg(v) = arg {
                            let r = if f.cls[*v] == Cls::Int { ARGS.get(ints).copied() } else { Some(flts as u8) };
                            if f.cls[*v] == Cls::Int { ints += 1 } else { flts += 1 }
                            hint[*v] = hint[*v].or(r);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut order: Vec<V> = (0..n).filter(|&v| start[v] != usize::MAX).collect();
    order.sort_by_key(|&v| start[v]);
    let mut loc = vec![Loc::Slot(0); n];
    let mut frame = (f.frame + 7) / 8 * 8;
    let mut new_slot = || {
        frame += 8;
        Loc::Slot(-frame)
    };
    let mut active: Vec<V> = vec![];
    let mut free = [INT_POOL.iter().fold(0u32, |m, r| m | 1 << r), FLT_POOL.fold(0u32, |m, r| m | 1 << r)];
    for v in order {
        let c = f.cls[v] as usize;
        // Registers whose intervals have ended are free again. One that ends
        // where this one starts is free too, as an instruction reads its operands
        // before it writes its result; unless it also starts there, being another
        // result of the same instruction that happens not to be used.
        active.retain(|&a| {
            let Loc::Reg(r) = loc[a] else { unreachable!() };
            let ended = end[a] < start[v] || (end[a] == start[v] && start[a] < start[v]);
            if ended {
                free[f.cls[a] as usize] |= 1 << r;
            }
            !ended
        });
        let allowed = match (f.cls[v], crosses(v, &calls), crosses(v, &strs)) {
            (Cls::Flt, true, _) => 0,
            (Cls::Flt, false, _) => !0,
            (Cls::Int, call, str) => !(if call { CALLER_SAVED } else { 0 } | if str { 1 << SI | 1 << DI } else { 0 }),
        };
        let pool: Vec<u8> = if f.cls[v] == Cls::Int { INT_POOL.to_vec() } else { FLT_POOL.collect() };
        if let Some(&r) = hint[v].iter().chain(&pool).find(|&&r| (free[c] & allowed) >> r & 1 != 0) {
            free[c] &= !(1 << r);
            loc[v] = Loc::Reg(r);
            active.push(v);
            continue;
        }
        // No register: the least used interval goes to the frame, this one or
        // one that holds a register this one may use.
        let victim = active.iter().copied().filter(|&a| f.cls[a] == f.cls[v] && matches!(loc[a], Loc::Reg(r) if allowed >> r & 1 != 0)).min_by_key(|&a| weight[a]);
        match victim {
            Some(a) if weight[a] < weight[v] => {
                loc[v] = loc[a];
                loc[a] = new_slot();
                active.retain(|&x| x != a);
                active.push(v);
            }
            _ => loc[v] = new_slot(),
        }
    }
    (loc, frame, start, end)
}

struct Emit<'a> {
    out: String,
    f: &'a Func,
    loc: Vec<Loc>,
    id: usize,       // makes the labels of this function unique
    next: Option<B>, // the block emitted after the current one
    tables: usize,   // jump tables emitted so far
}

fn fits32(c: i64) -> bool {
    c == c as i32 as i64
}

fn slot(off: i64) -> String {
    format!("{}(%rbp)", off)
}

/// The condition code of an integer comparison, and of its negation.
fn cc(op: Op, uns: bool) -> (&'static str, &'static str) {
    match (op, uns) {
        (Op::Eq, _) => ("e", "ne"),
        (Op::Ne, _) => ("ne", "e"),
        (Op::Lt, false) => ("l", "ge"),
        (Op::Lt, true) => ("b", "ae"),
        (_, false) => ("le", "g"),
        (_, true) => ("be", "a"),
    }
}

impl<'a> Emit<'a> {
    fn r(&self, reg: u8, size: u8) -> String {
        reg_name(GP[reg as usize], size as i64)
    }
    fn label(&self, b: B) -> String {
        format!(".L.o.{}.{}", self.id, b)
    }
    fn is_int(&self, v: V) -> bool {
        self.f.cls[v] == Cls::Int
    }

    // ----- integer values -----

    /// The value as an operand of an instruction: its register or its slot.
    fn opnd(&self, v: V, size: u8) -> String {
        match self.loc[v] {
            Loc::Reg(r) => self.r(r, size),
            Loc::Slot(o) => slot(o),
        }
    }
    /// A register holding the value: its own, or `scratch` loaded from the slot.
    fn in_reg(&mut self, v: V, scratch: u8) -> u8 {
        match self.loc[v] {
            Loc::Reg(r) => r,
            Loc::Slot(o) => {
                emit!(self, "  mov {}, {}", slot(o), self.r(scratch, 8));
                scratch
            }
        }
    }
    /// The register to compute the value in; `done` then puts it in its place.
    fn target(&self, v: V, scratch: u8) -> u8 {
        match self.loc[v] {
            Loc::Reg(r) => r,
            Loc::Slot(_) => scratch,
        }
    }
    fn done(&mut self, v: V, t: u8) {
        match self.loc[v] {
            Loc::Reg(r) if r == t => {}
            Loc::Reg(r) => emit!(self, "  mov {}, {}", self.r(t, 8), self.r(r, 8)),
            Loc::Slot(o) => emit!(self, "  mov {}, {}", self.r(t, 8), slot(o)),
        }
    }
    fn load_into(&mut self, t: u8, v: V) {
        if self.loc[v] != Loc::Reg(t) {
            emit!(self, "  mov {}, {}", self.opnd(v, 8), self.r(t, 8));
        }
    }

    // ----- float values (in %xmm registers; a slot always holds 8 bytes) -----

    fn xopnd(&self, v: V) -> String {
        match self.loc[v] {
            Loc::Reg(r) => format!("%xmm{}", r),
            Loc::Slot(o) => slot(o),
        }
    }
    fn xin_reg(&mut self, v: V, scratch: u8) -> u8 {
        match self.loc[v] {
            Loc::Reg(r) => r,
            Loc::Slot(o) => {
                emit!(self, "  movsd {}, %xmm{}", slot(o), scratch);
                scratch
            }
        }
    }
    fn xdone(&mut self, v: V, t: u8) {
        self.mov_loc(self.loc[v], Loc::Reg(t), true);
    }
    fn xload_into(&mut self, t: u8, v: V) {
        self.mov_loc(Loc::Reg(t), self.loc[v], true);
    }

    fn mov_loc(&mut self, d: Loc, s: Loc, flt: bool) {
        match (d, s, flt) {
            _ if d == s => {}
            (Loc::Reg(d), Loc::Reg(s), false) => emit!(self, "  mov {}, {}", self.r(s, 8), self.r(d, 8)),
            (Loc::Reg(d), Loc::Slot(o), false) => emit!(self, "  mov {}, {}", slot(o), self.r(d, 8)),
            (Loc::Slot(o), Loc::Reg(s), false) => emit!(self, "  mov {}, {}", self.r(s, 8), slot(o)),
            (Loc::Reg(d), Loc::Reg(s), true) => emit!(self, "  movaps %xmm{}, %xmm{}", s, d),
            (Loc::Reg(d), Loc::Slot(o), true) => emit!(self, "  movsd {}, %xmm{}", slot(o), d),
            (Loc::Slot(o), Loc::Reg(s), true) => emit!(self, "  movsd %xmm{}, {}", s, slot(o)),
            (Loc::Slot(d), Loc::Slot(s), _) => emit!(self, "  mov {}, %rax\n  mov %rax, {}", slot(s), slot(d)), // not part of a parallel move
        }
    }

    /// Does the moves `dst <- src` as if all at once: no source is overwritten
    /// before it is read. A cycle is broken through a scratch register.
    fn parallel_move(&mut self, mut moves: Vec<(Loc, Loc)>, flt: bool) {
        moves.retain(|(d, s)| d != s);
        while !moves.is_empty() {
            match (0..moves.len()).find(|&i| !moves.iter().any(|m| m.1 == moves[i].0)) {
                // No pending move reads this destination: it can be written.
                Some(i) => {
                    let (d, s) = moves.remove(i);
                    self.mov_loc(d, s, flt);
                }
                // Only cycles are left: park one destination's value.
                None => {
                    let (d, scratch) = (moves[0].0, Loc::Reg(if flt { 15 } else { AX }));
                    self.mov_loc(scratch, d, flt);
                    moves.iter_mut().filter(|m| m.1 == d).for_each(|m| m.1 = scratch);
                }
            }
        }
    }

    /// The text of a memory operand. A base or an index that lives in the
    /// frame is loaded into %rcx or %rdx.
    fn mem(&mut self, a: &Addr) -> String {
        let disp = if a.disp == 0 { String::new() } else { a.disp.to_string() };
        let index = a.index.map(|(v, scale)| {
            let r = self.in_reg(v, DX);
            format!(",{},{}", self.r(r, 8), scale)
        });
        let index = index.unwrap_or_default();
        match &a.base {
            Base::Rbp => format!("{}(%rbp{})", a.disp, index),
            Base::Sym(name) if a.disp == 0 => format!("{}(%rip)", name),
            Base::Sym(name) => format!("{}{:+}(%rip)", name, a.disp),
            Base::Reg(v) => {
                let r = self.in_reg(*v, CX);
                format!("{}({}{})", disp, self.r(r, 8), index)
            }
        }
    }

    /// Sets the flags of `a - b`.
    fn cmp(&mut self, a: V, b: Opd, w64: bool) {
        let (size, sfx) = if w64 { (8, 'q') } else { (4, 'l') };
        let btext = match b {
            Opd::Imm(c) => format!("${}", c),
            Opd::V(bv) => self.opnd(bv, size),
        };
        match (self.loc[a], b) {
            (Loc::Reg(r), _) => emit!(self, "  cmp {}, {}", btext, self.r(r, size)),
            (Loc::Slot(o), Opd::V(bv)) if matches!(self.loc[bv], Loc::Slot(_)) => {
                emit!(self, "  mov {}, %rax\n  cmp {}, {}", slot(o), btext, self.r(AX, size));
            }
            (Loc::Slot(o), _) => emit!(self, "  cmp{} {}, {}", sfx, btext, slot(o)),
        }
    }

    /// Puts the 0 or 1 of a `set` condition (in %al) into an integer value.
    fn set_result(&mut self, d: V) {
        match self.loc[d] {
            Loc::Reg(r) => emit!(self, "  movzbl %al, {}", self.r(r, 4)),
            Loc::Slot(o) => emit!(self, "  movzbl %al, %eax\n  mov %rax, {}", slot(o)),
        }
    }

    fn bin(&mut self, op: Op, d: V, a: V, b: Opd, w64: bool, uns: bool) {
        let (size, sfx) = if w64 { (8, 'q') } else { (4, 'l') };
        match op {
            Op::Eq | Op::Ne | Op::Lt | Op::Le => {
                self.cmp(a, b, w64);
                emit!(self, "  set{} %al", cc(op, uns).0);
                self.set_result(d);
            }
            Op::Div | Op::Mod => {
                self.load_into(AX, a);
                let divisor = match b {
                    Opd::V(bv) => self.opnd(bv, size),
                    Opd::Imm(c) => {
                        emit!(self, "  mov ${}, {}", c, self.r(CX, size));
                        self.r(CX, size)
                    }
                };
                if uns {
                    emit!(self, "  xor %edx, %edx\n  div{} {}", sfx, divisor);
                } else {
                    emit!(self, "  {}\n  idiv{} {}", if w64 { "cqto" } else { "cltd" }, sfx, divisor);
                }
                self.done(d, if op == Op::Div { AX } else { DX });
            }
            Op::Shl | Op::Shr => {
                let count = match b {
                    Opd::Imm(c) => format!("${}", c & 63),
                    Opd::V(bv) => {
                        emit!(self, "  mov {}, %ecx", self.opnd(bv, 4));
                        "%cl".to_string()
                    }
                };
                let t = self.target(d, AX);
                self.load_into(t, a);
                emit!(self, "  {} {}, {}", if op == Op::Shl { "shl" } else if uns { "shr" } else { "sar" }, count, self.r(t, size));
                self.done(d, t);
            }
            _ => {
                // t = a; t op= b. If t is the register of b, a would overwrite it:
                // the operands are swapped, or the result goes through %rax.
                let (mut t, mut a, mut b) = (self.target(d, AX), a, b);
                if let Opd::V(bv) = b {
                    if self.loc[bv] == Loc::Reg(t) && self.loc[a] != Loc::Reg(t) {
                        if op == Op::Sub {
                            t = AX;
                        } else {
                            (a, b) = (bv, Opd::V(a));
                        }
                    }
                }
                let (ra, tn) = (self.opnd(a, 8), self.r(t, size));
                match (op, b, self.loc[a]) {
                    // Three-operand forms: the result need not start as a copy of a.
                    (Op::Mul, Opd::Imm(c), _) => {
                        emit!(self, "  imul ${}, {}, {}", c, self.opnd(a, size), tn);
                        return self.done(d, t);
                    }
                    (Op::Add, Opd::Imm(c), Loc::Reg(r)) if r != t => {
                        emit!(self, "  lea {}({}), {}", c, ra, tn);
                        return self.done(d, t);
                    }
                    (Op::Add, Opd::V(bv), Loc::Reg(r)) if r != t && matches!(self.loc[bv], Loc::Reg(_)) => {
                        emit!(self, "  lea ({},{}), {}", ra, self.opnd(bv, 8), tn);
                        return self.done(d, t);
                    }
                    _ => {}
                }
                self.load_into(t, a);
                let (src, t_name) = (
                    match b {
                        Opd::Imm(c) => format!("${}", c),
                        Opd::V(bv) => self.opnd(bv, size),
                    },
                    self.r(t, size),
                );
                match (op, b) {
                    (Op::Mul, Opd::Imm(_)) => emit!(self, "  imul {}, {}, {}", src, t_name, t_name),
                    (Op::Mul, _) => emit!(self, "  imul {}, {}", src, t_name),
                    (Op::Add, _) => emit!(self, "  add {}, {}", src, t_name),
                    (Op::Sub, _) => emit!(self, "  sub {}, {}", src, t_name),
                    (Op::BitAnd, _) => emit!(self, "  and {}, {}", src, t_name),
                    (Op::BitOr, _) => emit!(self, "  or {}, {}", src, t_name),
                    _ => emit!(self, "  xor {}, {}", src, t_name),
                }
                self.done(d, t);
            }
        }
    }

    fn fbin(&mut self, op: Op, d: V, a: V, b: V, size: u8) {
        let s = if size == 4 { "ss" } else { "sd" };
        match op {
            Op::Eq | Op::Ne | Op::Lt | Op::Le => {
                // The flags of b compared with a, as in codegen.rs.
                let rb = self.xin_reg(b, 1);
                emit!(self, "  ucomi{} {}, %xmm{}", s, self.xopnd(a), rb);
                match op {
                    Op::Eq => emit!(self, "  sete %al\n  setnp %dl\n  and %dl, %al"),
                    Op::Ne => emit!(self, "  setne %al\n  setp %dl\n  or %dl, %al"),
                    Op::Lt => emit!(self, "  seta %al"),
                    _ => emit!(self, "  setae %al"),
                }
                self.set_result(d);
            }
            _ => {
                let mut t = match self.loc[d] {
                    Loc::Reg(r) => r,
                    Loc::Slot(_) => 0,
                };
                if self.loc[b] == Loc::Reg(t) && self.loc[a] != Loc::Reg(t) {
                    t = 0;
                }
                self.xload_into(t, a);
                let ins = match op {
                    Op::Add => "add",
                    Op::Sub => "sub",
                    Op::Mul => "mul",
                    _ => "div",
                };
                emit!(self, "  {}{} {}, %xmm{}", ins, s, self.xopnd(b), t);
                self.xdone(d, t);
            }
        }
    }

    fn call(&mut self, dsts: &[V], callee: &Option<V>, name: &str, args: &[Arg], stack: i64) {
        let area = (stack + 15) / 16 * 16;
        if area > 0 {
            emit!(self, "  sub ${}, %rsp", area);
        }
        // Stack arguments first, while every value is still where it lives; then
        // the register arguments, all at once.
        let (mut ints, mut flts) = (vec![], vec![]);
        for arg in args {
            match arg {
                Arg::Reg(v) if self.is_int(*v) => ints.push((Loc::Reg(ARGS[ints.len()]), self.loc[*v])),
                Arg::Reg(v) => flts.push((Loc::Reg(flts.len() as u8), self.loc[*v])),
                Arg::Stack(v, at) if self.is_int(*v) => {
                    let r = self.in_reg(*v, AX);
                    emit!(self, "  mov {}, {}(%rsp)", self.r(r, 8), at);
                }
                Arg::Stack(v, at) => {
                    let r = self.xin_reg(*v, 0);
                    emit!(self, "  movsd %xmm{}, {}(%rsp)", r, at);
                }
                Arg::Copy(v, at, size) => {
                    let src = self.in_reg(*v, CX);
                    let mut k = 0;
                    for step in [8, 4, 2, 1] {
                        while size - k >= step {
                            let (ax, src) = (self.r(AX, step as u8), self.r(src, 8));
                            emit!(self, "  mov {}({}), {}\n  mov {}, {}(%rsp)", k, src, ax, ax, at + k);
                            k += step;
                        }
                    }
                }
            }
        }
        if let Some(c) = callee {
            ints.push((Loc::Reg(R11), self.loc[*c])); // not an argument register
        }
        let nflt = flts.len();
        self.parallel_move(ints, false);
        self.parallel_move(flts, true);
        emit!(self, "  mov ${}, %eax", nflt); // for a variadic callee
        match callee {
            Some(_) => emit!(self, "  call *%r11"),
            None => emit!(self, "  call {}@PLT", name),
        }
        if area > 0 {
            emit!(self, "  add ${}, %rsp", area);
        }
        let (mut int_regs, mut flt_regs) = ([AX, DX].into_iter(), [0, 1].into_iter());
        for &d in dsts {
            if self.is_int(d) {
                self.done(d, int_regs.next().unwrap());
            } else {
                self.xdone(d, flt_regs.next().unwrap());
            }
        }
    }

    fn ins(&mut self, ins: &Ins) {
        match ins {
            Ins::Entry(params) => {
                let (ints, flts): (Vec<_>, Vec<_>) = params.iter().partition(|(v, _)| self.is_int(*v));
                let ints = ints.iter().map(|(v, i)| (self.loc[*v], Loc::Reg(ARGS[*i]))).collect();
                let flts = flts.iter().map(|(v, i)| (self.loc[*v], Loc::Reg(*i as u8))).collect();
                self.parallel_move(ints, false);
                self.parallel_move(flts, true);
            }
            Ins::Imm(d, c) => match self.loc[*d] {
                Loc::Reg(r) if *c == 0 => emit!(self, "  xor {}, {}", self.r(r, 4), self.r(r, 4)),
                Loc::Reg(r) if *c == *c as u32 as i64 => emit!(self, "  mov ${}, {}", c, self.r(r, 4)),
                Loc::Reg(r) if *c == *c as i32 as i64 => emit!(self, "  mov ${}, {}", c, self.r(r, 8)),
                Loc::Reg(r) => emit!(self, "  movabs ${}, {}", c, self.r(r, 8)),
                Loc::Slot(o) if *c == *c as i32 as i64 => emit!(self, "  movq ${}, {}", c, slot(o)),
                Loc::Slot(o) => emit!(self, "  movabs ${}, %rax\n  mov %rax, {}", c, slot(o)),
            },
            Ins::FImm(d, bits, size) => match self.loc[*d] {
                Loc::Reg(r) if *bits == 0 => emit!(self, "  pxor %xmm{}, %xmm{}", r, r),
                Loc::Reg(r) if *size == 4 => emit!(self, "  mov ${}, %eax\n  movd %eax, %xmm{}", bits, r),
                Loc::Reg(r) => emit!(self, "  movabs ${}, %rax\n  movq %rax, %xmm{}", *bits as i64, r),
                Loc::Slot(o) => emit!(self, "  movabs ${}, %rax\n  mov %rax, {}", *bits as i64, slot(o)),
            },
            Ins::Mov(d, s) => self.mov_loc(self.loc[*d], self.loc[*s], !self.is_int(*d)),
            Ins::Bin(op, d, a, b, w64, uns) => self.bin(*op, *d, *a, *b, *w64, *uns),
            Ins::Neg(d, s) | Ins::Not(d, s) => {
                let t = self.target(*d, AX);
                self.load_into(t, *s);
                emit!(self, "  {} {}", if matches!(ins, Ins::Neg(..)) { "neg" } else { "not" }, self.r(t, 8));
                self.done(*d, t);
            }
            Ins::FBin(op, d, a, b, size) => self.fbin(*op, *d, *a, *b, *size),
            Ins::FNeg(d, s, size) => {
                let t = match self.loc[*d] {
                    Loc::Reg(r) => r,
                    Loc::Slot(_) => 0,
                };
                self.xload_into(t, *s);
                if *size == 4 {
                    emit!(self, "  mov $0x80000000, %eax\n  movd %eax, %xmm1\n  xorps %xmm1, %xmm{}", t);
                } else {
                    emit!(self, "  movabs $0x8000000000000000, %rax\n  movq %rax, %xmm1\n  xorpd %xmm1, %xmm{}", t);
                }
                self.xdone(*d, t);
            }
            Ins::Ext(d, s, from, signed) => {
                let t = self.target(*d, AX);
                let x = if *signed { 's' } else { 'z' };
                match from {
                    1 => emit!(self, "  mov{}bl {}, {}", x, self.opnd(*s, 1), self.r(t, 4)),
                    2 => emit!(self, "  mov{}wl {}, {}", x, self.opnd(*s, 2), self.r(t, 4)),
                    _ if *signed => emit!(self, "  movslq {}, {}", self.opnd(*s, 4), self.r(t, 8)),
                    _ => emit!(self, "  mov {}, {}", self.opnd(*s, 4), self.r(t, 4)),
                }
                self.done(*d, t);
            }
            Ins::F2F(d, s, to) => {
                let t = match self.loc[*d] {
                    Loc::Reg(r) => r,
                    Loc::Slot(_) => 0,
                };
                emit!(self, "  {} {}, %xmm{}", if *to == 8 { "cvtss2sd" } else { "cvtsd2ss" }, self.xopnd(*s), t);
                self.xdone(*d, t);
            }
            Ins::I2F(d, s, to, kind) => {
                let t = match self.loc[*d] {
                    Loc::Reg(r) => r,
                    Loc::Slot(_) => 0,
                };
                let x = if *to == 4 { "ss" } else { "sd" };
                match kind {
                    IntKind::I32 => emit!(self, "  cvtsi2{}l {}, %xmm{}", x, self.opnd(*s, 4), t),
                    IntKind::I64 => emit!(self, "  cvtsi2{}q {}, %xmm{}", x, self.opnd(*s, 8), t),
                    IntKind::U32 => emit!(self, "  mov {}, %eax\n  cvtsi2{}q %rax, %xmm{}", self.opnd(*s, 4), x, t),
                    IntKind::U64 => {
                        // The conversion is signed: a value with the top bit set is halved
                        // (keeping the lowest bit for rounding), converted and doubled.
                        self.load_into(AX, *s);
                        emit!(self, "  test %rax, %rax\n  js 1f\n  cvtsi2{}q %rax, %xmm{}\n  jmp 2f", x, t);
                        emit!(self, "1:\n  mov %rax, %rcx\n  and $1, %eax\n  shr %rcx\n  or %rax, %rcx");
                        emit!(self, "  cvtsi2{}q %rcx, %xmm{}\n  add{} %xmm{}, %xmm{}\n2:", x, t, x, t, t);
                    }
                }
                self.xdone(*d, t);
            }
            Ins::F2I(d, s, from, kind) => {
                self.xload_into(0, *s);
                if *from == 4 {
                    emit!(self, "  cvtss2sd %xmm0, %xmm0");
                }
                match kind {
                    IntKind::I32 => emit!(self, "  cvttsd2sil %xmm0, %eax"),
                    IntKind::U64 => {
                        // Values from 2^63 up do not fit a signed conversion.
                        emit!(self, "  movabs $0x43e0000000000000, %rcx\n  movq %rcx, %xmm1\n  ucomisd %xmm1, %xmm0\n  jae 1f");
                        emit!(self, "  cvttsd2siq %xmm0, %rax\n  jmp 2f\n1:\n  subsd %xmm1, %xmm0\n  cvttsd2siq %xmm0, %rax\n  btc $63, %rax\n2:");
                    }
                    _ => emit!(self, "  cvttsd2siq %xmm0, %rax"),
                }
                self.done(*d, AX);
            }
            Ins::Load(d, a, size, signed, _) => {
                let m = self.mem(a);
                if !self.is_int(*d) {
                    let t = match self.loc[*d] {
                        Loc::Reg(r) => r,
                        Loc::Slot(_) => 0,
                    };
                    emit!(self, "  {} {}, %xmm{}", if *size == 4 { "movss" } else { "movsd" }, m, t);
                    return self.xdone(*d, t);
                }
                let t = self.target(*d, AX);
                let x = if *signed { 's' } else { 'z' };
                match size {
                    1 => emit!(self, "  mov{}bl {}, {}", x, m, self.r(t, 4)),
                    2 => emit!(self, "  mov{}wl {}, {}", x, m, self.r(t, 4)),
                    4 => emit!(self, "  mov {}, {}", m, self.r(t, 4)),
                    _ => emit!(self, "  mov {}, {}", m, self.r(t, 8)),
                }
                self.done(*d, t);
            }
            Ins::Store(s, a, size) => {
                if self.is_int(*s) {
                    let r = self.in_reg(*s, AX);
                    let m = self.mem(a);
                    emit!(self, "  mov {}, {}", self.r(r, *size), m);
                } else {
                    let r = self.xin_reg(*s, 0);
                    let m = self.mem(a);
                    emit!(self, "  {} %xmm{}, {}", if *size == 4 { "movss" } else { "movsd" }, r, m);
                }
            }
            Ins::StoreImm(c, a, size) => {
                let m = self.mem(a);
                emit!(self, "  mov{} ${}, {}", ['b', 'w', ' ', 'l', ' ', ' ', ' ', 'q'][*size as usize - 1], c, m);
            }
            Ins::Lea(d, a) => {
                let (m, t) = (self.mem(a), self.target(*d, AX));
                emit!(self, "  lea {}, {}", m, self.r(t, 8));
                self.done(*d, t);
            }
            Ins::GotAddr(d, name) | Ins::TlsAddr(d, name) => {
                let t = self.target(*d, AX);
                match ins {
                    Ins::GotAddr(..) => emit!(self, "  mov {}@GOTPCREL(%rip), {}", name, self.r(t, 8)),
                    // Initial-exec TLS model, as in codegen.rs.
                    _ => emit!(self, "  mov {}@gottpoff(%rip), {}\n  add %fs:0, {}", name, self.r(t, 8), self.r(t, 8)),
                }
                self.done(*d, t);
            }
            Ins::Copy(d, s, size) if *size <= 64 => {
                let (rd, rs) = (self.in_reg(*d, CX), self.in_reg(*s, DX));
                let mut k = 0;
                for step in [8, 4, 2, 1] {
                    while size - k >= step {
                        let (ax, rs, rd) = (self.r(AX, step as u8), self.r(rs, 8), self.r(rd, 8));
                        emit!(self, "  mov {}({}), {}\n  mov {}, {}({})", k, rs, ax, ax, k, rd);
                        k += step;
                    }
                }
            }
            Ins::Copy(d, s, size) => {
                self.parallel_move(vec![(Loc::Reg(DI), self.loc[*d]), (Loc::Reg(SI), self.loc[*s])], false);
                emit!(self, "  mov ${}, %ecx\n  rep movsb", size);
            }
            Ins::Zero(a, size) if *size <= 64 => {
                let mut k = 0;
                for (step, mov) in [(8, "movq"), (4, "movl"), (2, "movw"), (1, "movb")] {
                    while size - k >= step {
                        let m = self.mem(&Addr { disp: a.disp + k, ..a.clone() });
                        emit!(self, "  {} $0, {}", mov, m);
                        k += step;
                    }
                }
            }
            Ins::Zero(a, size) => {
                let m = self.mem(a);
                emit!(self, "  lea {}, %rdi\n  mov ${}, %ecx\n  xor %eax, %eax\n  rep stosb", m, size);
            }
            Ins::Call(dsts, callee, name, args, stack) => self.call(dsts, callee, name, args, *stack),
            Ins::Loc(file, line) => emit!(self, "  .loc {} {}", file + 1, line),
            Ins::Jmp(b) => {
                if Some(*b) != self.next {
                    emit!(self, "  jmp {}", self.label(*b));
                }
            }
            Ins::Br(op, a, b, w64, uns, t, f) => {
                self.cmp(*a, *b, *w64);
                let (yes, no) = cc(*op, *uns);
                if Some(*t) == self.next {
                    emit!(self, "  j{} {}", no, self.label(*f));
                } else {
                    emit!(self, "  j{} {}", yes, self.label(*t));
                    if Some(*f) != self.next {
                        emit!(self, "  jmp {}", self.label(*f));
                    }
                }
            }
            Ins::Switch(v, w64, cases, default) => {
                let r = self.in_reg(*v, AX);
                // The case values as the comparisons see them: at the width of the value.
                let at = |c: i64| if *w64 { c } else { c as i32 as i64 };
                let (min, max) = (cases.iter().map(|c| at(c.0)).min().unwrap_or(0), cases.iter().map(|c| at(c.1)).max().unwrap_or(0));
                let (span, covered) = (max as i128 - min as i128 + 1, cases.iter().map(|c| at(c.1) as i128 - at(c.0) as i128 + 1).sum::<i128>());
                if cases.len() >= 5 && span <= 3 * covered && span <= 1 << 16 && cases.iter().all(|c| at(c.0) <= at(c.1)) {
                    // Dense cases: one bounds check, then a jump through a table of
                    // offsets (relative to the table, so it needs no relocation).
                    self.tables += 1;
                    let (table, size) = (format!(".L.o.{}.t{}", self.id, self.tables), if *w64 { 8 } else { 4 });
                    emit!(self, "  mov {}, {}", self.r(r, size), self.r(AX, size));
                    if min == 0 {
                    } else if fits32(min) {
                        emit!(self, "  sub ${}, {}", min, self.r(AX, size));
                    } else {
                        emit!(self, "  movabs ${}, %rcx\n  sub %rcx, %rax", min);
                    }
                    emit!(self, "  cmp ${}, {}\n  ja {}", span - 1, self.r(AX, size), self.label(*default));
                    emit!(self, "  lea {}(%rip), %rcx\n  movslq (%rcx,%rax,4), %rax\n  add %rcx, %rax\n  jmp *%rax", table);
                    emit!(self, "  .pushsection .rodata\n  .balign 4\n{}:", table);
                    for x in min..=max {
                        let b = cases.iter().find(|c| at(c.0) <= x && x <= at(c.1)).map_or(*default, |c| c.2);
                        emit!(self, "  .long {}-{}", self.label(b), table);
                    }
                    emit!(self, "  .popsection");
                    return;
                }
                for (lo, hi, b) in cases {
                    let l = self.label(*b);
                    match (*w64, lo == hi) {
                        (false, true) => emit!(self, "  cmp ${}, {}\n  je {}", *lo as i32, self.r(r, 4), l),
                        // A range: (x - lo) <= (hi - lo), unsigned.
                        (false, false) => emit!(self, "  mov {}, %ecx\n  sub ${}, %ecx\n  cmp ${}, %ecx\n  jbe {}", self.r(r, 4), *lo as i32, hi.wrapping_sub(*lo) as i32, l),
                        (true, true) if *lo == *lo as i32 as i64 => emit!(self, "  cmp ${}, {}\n  je {}", lo, self.r(r, 8), l),
                        (true, true) => emit!(self, "  movabs ${}, %rcx\n  cmp %rcx, {}\n  je {}", lo, self.r(r, 8), l),
                        (true, false) => {
                            emit!(self, "  mov {}, %rcx\n  movabs ${}, %rdx\n  sub %rdx, %rcx", self.r(r, 8), lo);
                            emit!(self, "  movabs ${}, %rdx\n  cmp %rdx, %rcx\n  jbe {}", hi.wrapping_sub(*lo), l);
                        }
                    }
                }
                if Some(*default) != self.next {
                    emit!(self, "  jmp {}", self.label(*default));
                }
            }
            Ins::Ret(vs) => {
                let (mut int_regs, mut flt_regs) = ([AX, DX].into_iter(), [0, 1].into_iter());
                for &v in vs {
                    if self.is_int(v) {
                        self.load_into(int_regs.next().unwrap(), v);
                    } else {
                        self.xload_into(flt_regs.next().unwrap(), v);
                    }
                }
                if self.next.is_some() {
                    emit!(self, "  jmp .L.o.{}.ret", self.id);
                }
            }
        }
    }
}

/// The code of a function, from the prologue to `ret`.
pub fn emit(f: &Func, id: usize) -> String {
    let (mut loc, frame, start, end) = allocate(f);
    // A value computed only to be returned is computed in the return register
    // (%rax or %xmm0: both are register 0, where the emitter computes anyway
    // when a result lives in the frame).
    let mut pos = 0;
    for &b in &f.layout {
        if let [.., last, Ins::Ret(vs)] = f.blocks[b].as_slice() {
            if let [d] = vs.as_slice() {
                let (p, mut defined) = (pos + f.blocks[b].len() - 2, false);
                last.regs(&mut |v, def| defined |= def && v == *d);
                if defined && start[*d] == p && end[*d] == p + 1 {
                    loc[*d] = Loc::Reg(0);
                }
            }
        }
        pos += f.blocks[b].len();
    }
    // The callee-saved registers in use are kept in the frame.
    let mut saved: Vec<(u8, i64)> = vec![];
    let mut frame = frame;
    for &r in &INT_POOL[6..] {
        if loc.iter().zip(&f.cls).any(|(l, c)| *c == Cls::Int && *l == Loc::Reg(r)) {
            frame += 8;
            saved.push((r, -frame));
        }
    }
    let mut e = Emit { out: String::new(), f, loc, id, next: None, tables: 0 };
    // The .cfi directives describe the frame for unwinders and debuggers.
    emit!(e, "  .cfi_startproc\n  push %rbp\n  .cfi_def_cfa_offset 16\n  .cfi_offset %rbp, -16\n  mov %rsp, %rbp\n  .cfi_def_cfa_register %rbp");
    if frame > 0 {
        emit!(e, "  sub ${}, %rsp", (frame + 15) / 16 * 16);
    }
    for (r, off) in &saved {
        emit!(e, "  mov {}, {}\n  .cfi_offset {}, {}", e.r(*r, 8), slot(*off), e.r(*r, 8), off - 16);
    }
    for (i, &b) in f.layout.iter().enumerate() {
        e.next = f.layout.get(i + 1).copied();
        emit!(e, "{}:", e.label(b));
        for ins in &f.blocks[b] {
            e.ins(ins);
        }
    }
    emit!(e, ".L.o.{}.ret:", id);
    for (r, off) in &saved {
        emit!(e, "  mov {}, {}", slot(*off), e.r(*r, 8));
    }
    emit!(e, "  mov %rbp, %rsp\n  pop %rbp\n  .cfi_def_cfa %rsp, 8\n  ret\n  .cfi_endproc");
    e.out
}
