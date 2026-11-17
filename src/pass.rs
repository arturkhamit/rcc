// Optimization passes on the IR of ir.rs, run on a function before its
// registers are allocated.
//
// Most of them rest on one fact: a virtual register that exactly one
// instruction defines has the same value wherever it is used. (That
// definition is on every path to a use: `lower` gives a register that could
// be read before it is written a second definition, at the entry.)
use crate::ir::*;
use crate::parse::Op;
use std::collections::{HashMap, HashSet};

/// Optimizes every function of a translation unit. `order` lists them as
/// defined; `inlinable` are those whose definition here is the one a call
/// reaches (not weak, not replaceable when the unit goes into a shared library).
pub fn optimize_all<'a>(funcs: &mut HashMap<&'a str, Func>, order: &[&'a str], inlinable: &HashSet<&'a str>) {
    let mut seen = HashSet::new();
    for name in order {
        prepare(name, funcs, inlinable, &mut seen);
    }
}

/// Optimizes one function, after the functions it calls: their bodies, in
/// their final form, take the place of the calls where they are small.
fn prepare<'a>(name: &'a str, funcs: &mut HashMap<&'a str, Func>, inlinable: &HashSet<&'a str>, seen: &mut HashSet<&'a str>) {
    if !seen.insert(name) {
        return;
    }
    // While the function is worked on it is not in the map, so a call that
    // leads back to it (recursion) finds nothing to inline.
    let Some(mut f) = funcs.remove(name) else { return };
    let mut callees = vec![];
    for &b in &f.layout {
        for ins in &f.blocks[b] {
            if let Ins::Call(_, None, callee, ..) = ins {
                callees.extend(funcs.get_key_value(callee.as_str()).map(|(k, _)| *k));
            }
        }
    }
    for callee in callees {
        prepare(callee, funcs, inlinable, seen);
    }
    inline(&mut f, funcs, inlinable);
    optimize(&mut f);
    funcs.insert(name, f);
}

/// A function of at most this many instructions is inlined wherever it is called.
const INLINE_SIZE: usize = 30;
/// A function grows by at most this many instructions through inlining.
const INLINE_GROWTH: usize = 2000;

/// Replaces calls of small functions by their bodies.
fn inline(f: &mut Func, funcs: &HashMap<&str, Func>, inlinable: &HashSet<&str>) {
    let mut budget = INLINE_GROWTH;
    let mut i = 0;
    while i < f.layout.len() {
        let b = f.layout[i];
        i += 1;
        // The first call in the block whose callee can be inlined, with the
        // registers that hold the integer and the float arguments.
        let found = f.blocks[b].iter().enumerate().find_map(|(k, ins)| {
            let Ins::Call(_, None, name, args, 0) = ins else { return None };
            let g = funcs.get(name.as_str()).filter(|_| inlinable.contains(name.as_str()))?;
            let size: usize = g.layout.iter().map(|b| g.blocks[*b].len()).sum();
            if size > INLINE_SIZE.min(budget) || g.frame > 256 {
                return None;
            }
            let (mut ints, mut flts) = (vec![], vec![]);
            for arg in args {
                let Arg::Reg(v) = arg else { return None };
                if f.cls[*v] == Cls::Int { ints.push(*v) } else { flts.push(*v) }
            }
            // Every parameter the callee reads must have been passed, in a register.
            let Some(Ins::Entry(params)) = g.blocks[g.layout[0]].first() else { return None };
            let passed = params.iter().all(|(v, i)| *i < if g.cls[*v] == Cls::Int { ints.len() } else { flts.len() });
            // A parameter passed on the stack is read from the caller's frame, which an inlined body does not have.
            let on_stack = g.layout.iter().flat_map(|b| &g.blocks[*b]).any(|ins| matches!(ins.clone().addr_mut(), Some(Addr { base: Base::Rbp, disp: 0.., .. })));
            (passed && !on_stack).then_some((k, g, ints, flts, size))
        });
        let Some((k, g, ints, flts, size)) = found else { continue };
        budget -= size;

        // The block ends at the call; what followed it goes on in a new block.
        let rest = f.blocks[b].split_off(k + 1);
        let Some(Ins::Call(dsts, ..)) = f.blocks[b].pop() else { unreachable!() };
        let (v0, b0) = (f.cls.len(), f.blocks.len());
        let after = b0 + g.blocks.len();
        f.cls.extend(&g.cls);
        // The callee's variables in memory get their own part of the frame.
        let shift = (f.frame + 15) / 16 * 16;
        f.frame = shift + g.frame;
        for block in &g.blocks {
            let mut new = vec![];
            for ins in block {
                let mut ins = ins.clone();
                ins.regs_mut(&mut |v, _| *v += v0);
                ins.targets_mut(&mut |t| *t += b0);
                if let Some(Addr { base: Base::Rbp, disp, .. }) = ins.addr_mut() {
                    *disp -= shift;
                }
                match ins {
                    // The parameters are copies of the arguments.
                    Ins::Entry(params) => new.extend(params.iter().map(|(p, i)| Ins::Mov(*p, if f.cls[*p] == Cls::Int { ints[*i] } else { flts[*i] }))),
                    // The results go to the registers the call put them in.
                    Ins::Ret(vs) => {
                        for cls in [Cls::Int, Cls::Flt] {
                            let pairs = dsts.iter().filter(|d| f.cls[**d] == cls).zip(vs.iter().filter(|v| f.cls[**v] == cls));
                            new.extend(pairs.map(|(d, v)| Ins::Mov(*d, *v)));
                        }
                        new.push(Ins::Jmp(after));
                    }
                    ins => new.push(ins),
                }
            }
            f.blocks.push(new);
        }
        f.blocks[b].push(Ins::Jmp(b0 + g.layout[0]));
        f.blocks.push(rest);
        f.layout.splice(i..i, g.layout.iter().map(|b| b0 + b).chain([after]));
        // On to what followed the call. The body itself is not looked at again:
        // the callee has had its own inlining, and it may call itself.
        i += g.layout.len();
    }
}

pub fn optimize(f: &mut Func) {
    // Each pass leaves work for the others; a few rounds settle nearly everything.
    for _ in 0..8 {
        let mut changed = fold(f);
        changed |= post_increment(f);
        changed |= reuse(f);
        changed |= remove_dead(f);
        changed |= tidy(f);
        if !changed {
            break;
        }
    }
}

/// How many instructions define each register, and how many use it.
fn counts(f: &Func) -> (Vec<u32>, Vec<u32>) {
    let (mut defs, mut uses) = (vec![0; f.cls.len()], vec![0; f.cls.len()]);
    for &b in &f.layout {
        for ins in &f.blocks[b] {
            ins.regs(&mut |v, def| if def { defs[v] += 1 } else { uses[v] += 1 });
        }
    }
    (defs, uses)
}

fn fits(c: i64) -> bool {
    c == c as i32 as i64
}

/// The result of an integer operation on constants, as the machine computes
/// it. None for a division by zero, which is left to happen at run time.
fn eval(op: Op, x: i64, y: i64, w64: bool, uns: bool) -> Option<i64> {
    if !w64 {
        let (a, b) = (x as i32, y as i32);
        let (ua, ub) = (a as u32, b as u32);
        return Some(match op {
            Op::Add => a.wrapping_add(b),
            Op::Sub => a.wrapping_sub(b),
            Op::Mul => a.wrapping_mul(b),
            Op::Div | Op::Mod if b == 0 => return None,
            Op::Div if uns => (ua / ub) as i32,
            Op::Div => a.wrapping_div(b),
            Op::Mod if uns => (ua % ub) as i32,
            Op::Mod => a.wrapping_rem(b),
            Op::BitAnd => a & b,
            Op::BitOr => a | b,
            Op::BitXor => a ^ b,
            Op::Shl => a.wrapping_shl(ub),
            Op::Shr if uns => ua.wrapping_shr(ub) as i32,
            Op::Shr => a.wrapping_shr(ub),
            Op::Eq => (a == b) as i32,
            Op::Ne => (a != b) as i32,
            Op::Lt => (if uns { ua < ub } else { a < b }) as i32,
            Op::Le => (if uns { ua <= ub } else { a <= b }) as i32,
        } as i64);
    }
    let (ux, uy) = (x as u64, y as u64);
    Some(match op {
        Op::Add => x.wrapping_add(y),
        Op::Sub => x.wrapping_sub(y),
        Op::Mul => x.wrapping_mul(y),
        Op::Div | Op::Mod if y == 0 => return None,
        Op::Div if uns => (ux / uy) as i64,
        Op::Div => x.wrapping_div(y),
        Op::Mod if uns => (ux % uy) as i64,
        Op::Mod => x.wrapping_rem(y),
        Op::BitAnd => x & y,
        Op::BitOr => x | y,
        Op::BitXor => x ^ y,
        Op::Shl => x.wrapping_shl(y as u32),
        Op::Shr if uns => ux.wrapping_shr(y as u32) as i64,
        Op::Shr => x.wrapping_shr(y as u32),
        Op::Eq => (x == y) as i64,
        Op::Ne => (x != y) as i64,
        Op::Lt => (if uns { ux < uy } else { x < y }) as i64,
        Op::Le => (if uns { ux <= uy } else { x <= y }) as i64,
    })
}

/// What a register defined once is known to be: a constant, a copy of another
/// such register, or such a register plus a constant.
#[derive(Clone, Copy)]
enum Val {
    Any,
    Const(i64),
    Same(V),
    Plus(V, i64, bool), // the width of the addition
}

/// A chain of copies ends at the register that is not a copy.
fn root(val: &[Val], mut v: V) -> V {
    for _ in 0..64 {
        match val[v] {
            Val::Same(s) => v = s,
            _ => break,
        }
    }
    v
}

fn konst(val: &[Val], v: V) -> Option<i64> {
    match val[v] {
        Val::Const(c) => Some(c),
        _ => None,
    }
}

/// Constant folding, and the propagation of constants and copies into the
/// instructions that use them.
fn fold(f: &mut Func) -> bool {
    let (ndef, _) = counts(f);
    let mut val = vec![Val::Any; f.cls.len()];
    let learn = |val: &mut [Val], ins: &Ins| match ins {
        Ins::Imm(d, c) if ndef[*d] == 1 => val[*d] = Val::Const(*c),
        Ins::Mov(d, s) if ndef[*d] == 1 && ndef[*s] == 1 && d != s => val[*d] = Val::Same(*s),
        Ins::Bin(op @ (Op::Add | Op::Sub), d, a, Opd::Imm(c), w64, _) if ndef[*d] == 1 && ndef[*a] == 1 && d != a => {
            val[*d] = Val::Plus(*a, if *op == Op::Add { *c } else { -*c }, *w64);
        }
        _ => {}
    };
    for &b in &f.layout {
        for ins in &f.blocks[b] {
            learn(&mut val, ins);
        }
    }

    let mut changed = false;
    for i in 0..f.layout.len() {
        let b = f.layout[i];
        for ins in &mut f.blocks[b] {
            ins.regs_mut(&mut |v, def| {
                if !def && root(&val, *v) != *v {
                    *v = root(&val, *v);
                    changed = true;
                }
            });
            let konst = |v: V| konst(&val, v);
            let opd = |o: &Opd| match o {
                Opd::Imm(c) => Some(*c),
                Opd::V(v) => konst(*v),
            };
            let new = match &*ins {
                Ins::Mov(d, s) if f.cls[*d] == Cls::Int => konst(*s).map(|c| Ins::Imm(*d, c)),
                Ins::Neg(d, s) => konst(*s).map(|c| Ins::Imm(*d, c.wrapping_neg())),
                Ins::Not(d, s) => konst(*s).map(|c| Ins::Imm(*d, !c)),
                Ins::Ext(d, s, from, signed) => konst(*s).map(|c| {
                    Ins::Imm(*d, match (from, signed) {
                        (1, true) => c as i8 as i64,
                        (1, false) => c as u8 as i64,
                        (2, true) => c as i16 as i64,
                        (2, false) => c as u16 as i64,
                        (_, true) => c as i32 as i64,
                        (_, false) => c as u32 as i64,
                    })
                }),
                Ins::Bin(op, d, a, b, w64, uns) => {
                    let (op, d, w64, uns) = (*op, *d, *w64, *uns);
                    let (ca, cb) = (konst(*a), opd(b));
                    // A constant operand becomes an immediate; on the left it
                    // changes sides first, if the operation allows.
                    let (na, nb) = match (ca, cb, *b) {
                        (_, Some(y), _) if fits(y) => (*a, Opd::Imm(y)),
                        (Some(x), None, Opd::V(bv)) if fits(x) && matches!(op, Op::Add | Op::Mul | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Eq | Op::Ne) => (bv, Opd::Imm(x)),
                        _ => (*a, *b),
                    };
                    match (op, nb, val[na]) {
                        _ if ca.is_some() && cb.is_some() && eval(op, ca.unwrap(), cb.unwrap(), w64, uns).is_some() => Some(Ins::Imm(d, eval(op, ca.unwrap(), cb.unwrap(), w64, uns).unwrap())),
                        (Op::Add | Op::Sub | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr, Opd::Imm(0), _) | (Op::Mul | Op::Div, Opd::Imm(1), _) | (Op::BitAnd, Opd::Imm(-1), _) => Some(Ins::Mov(d, na)),
                        (Op::Mul | Op::BitAnd, Opd::Imm(0), _) => Some(Ins::Imm(d, 0)),
                        // (x + c0) + c1 is x + (c0 + c1).
                        (Op::Add | Op::Sub, Opd::Imm(c1), Val::Plus(x, c0, w)) if w == w64 && fits(c0 + if op == Op::Add { c1 } else { -c1 }) => {
                            Some(Ins::Bin(Op::Add, d, x, Opd::Imm(c0 + if op == Op::Add { c1 } else { -c1 }), w64, uns))
                        }
                        _ if (na, nb) != (*a, *b) => Some(Ins::Bin(op, d, na, nb, w64, uns)),
                        _ => None,
                    }
                }
                Ins::Br(op, a, b, w64, uns, t, e) => match (konst(*a), opd(b), b) {
                    (Some(x), Some(y), _) => Some(Ins::Jmp(if eval(*op, x, y, *w64, *uns) == Some(1) { *t } else { *e })),
                    (None, Some(y), Opd::V(_)) if fits(y) => Some(Ins::Br(*op, *a, Opd::Imm(y), *w64, *uns, *t, *e)),
                    _ => None,
                },
                Ins::Switch(v, w64, cases, default) => konst(*v).map(|x| {
                    // As the emitted comparisons decide: at the width of the value, a range unsigned.
                    let hit = |lo: i64, hi: i64| match *w64 {
                        true => (x.wrapping_sub(lo) as u64) <= hi.wrapping_sub(lo) as u64,
                        false => (x.wrapping_sub(lo) as u32) <= hi.wrapping_sub(lo) as u32,
                    };
                    Ins::Jmp(cases.iter().find(|c| hit(c.0, c.1)).map_or(*default, |c| c.2))
                }),
                Ins::Store(v, a, size) if f.cls[*v] == Cls::Int => konst(*v).and_then(|c| {
                    let c = match size {
                        1 => c as i8 as i64,
                        2 => c as i16 as i64,
                        4 => c as i32 as i64,
                        _ => c,
                    };
                    fits(c).then(|| Ins::StoreImm(c, a.clone(), *size))
                }),
                _ => None,
            };
            if let Some(new) = new {
                *ins = new;
                changed = true;
                learn(&mut val, ins); // what it has become serves the instructions after it
            }
        }
    }
    changed
}

/// `x++` used as a value arrives as: x = x + c; t = x; r = t - c. The old
/// value is simply copied before the addition: r = x; x = x + c.
fn post_increment(f: &mut Func) -> bool {
    let (ndef, nuse) = counts(f);
    let mut changed = false;
    for i in 0..f.layout.len() {
        let block = &mut f.blocks[f.layout[i]];
        let mut k = 0;
        while k + 1 < block.len() {
            let found = match (&block[k], &block[k + 1]) {
                (Ins::Bin(op @ (Op::Add | Op::Sub), x, x0, Opd::Imm(c), w, _), Ins::Mov(t, x1)) if x == x0 && x == x1 && ndef[*t] == 1 && nuse[*t] == 1 => {
                    let (c, t, w, x) = (if *op == Op::Add { *c } else { -*c }, *t, *w, *x);
                    // The one use of t, if it undoes the addition.
                    let undo = block[k + 2..].iter().position(|ins| {
                        matches!(ins, Ins::Bin(op @ (Op::Add | Op::Sub), r, t1, Opd::Imm(c1), w1, _)
                            if *t1 == t && *w1 == w && ndef[*r] == 1 && (if *op == Op::Add { *c1 } else { -*c1 }) == -c)
                    });
                    undo.map(|j| (k + 2 + j, x))
                }
                _ => None,
            };
            if let Some((j, x)) = found {
                let Ins::Bin(_, r, ..) = block[j] else { unreachable!() };
                block.remove(j);
                block[k + 1] = std::mem::replace(&mut block[k], Ins::Mov(r, x));
                changed = true;
            }
            k += 1;
        }
    }
    changed
}

/// What an instruction computes, for finding one that computed the same.
#[derive(PartialEq, Eq, Hash, Clone)]
enum Key {
    Bin(Op, V, Opd, bool, bool),
    Un(u8, V, u8, u8), // the other operations of one operand, with their parameters
    FBin(Op, V, V, u8),
    Lea(Addr),
    Sym(bool, String),
    Load(Addr, u8, bool, bool), // size; signed; float
}

/// What is known along one path: the results of computations and of loads,
/// and for each register that may be written again the entries it is part of.
#[derive(Clone, Default)]
struct Known {
    pure: HashMap<Key, V>,
    loads: HashMap<Key, V>,
    with: HashMap<V, Vec<Key>>,
    same: HashMap<V, V>,        // a register that is a copy of another, neither written since
    copies: HashMap<V, Vec<V>>, // the copies of a register
}

impl Known {
    /// The register is written: nothing computed from it, or held in it, stands.
    fn forget(&mut self, v: V) {
        for key in self.with.remove(&v).unwrap_or_default() {
            self.pure.remove(&key);
            self.loads.remove(&key);
        }
        self.same.remove(&v);
        for copy in self.copies.remove(&v).unwrap_or_default() {
            self.same.remove(&copy);
        }
    }
}

/// Common subexpressions: an instruction that computes what an earlier one
/// computed becomes a copy of that result. "Earlier" is on the only path to
/// the instruction: in the same block, or in a chain of blocks each of which
/// is the only predecessor of the next. A load can be reused until memory is
/// written; a stored value stands for a later load of the same place.
fn reuse(f: &mut Func) -> bool {
    let (ndef, _) = counts(f);
    let mut preds: Vec<Vec<B>> = vec![vec![]; f.blocks.len()];
    for &b in &f.layout {
        for s in f.blocks[b].last().unwrap().succs() {
            if !preds[s].contains(&b) {
                preds[s].push(b);
            }
        }
    }
    let mut at_end: HashMap<B, Known> = HashMap::new();
    let mut changed = false;
    for i in 0..f.layout.len() {
        let b = f.layout[i];
        // Known at the start: what was known at the end of the only way in.
        let mut known = match preds[b].as_slice() {
            [p] if *p != b => at_end.get(p).cloned().unwrap_or_default(),
            _ => Known::default(),
        };
        for ins in &mut f.blocks[b] {
            // A copy is read as the register it was copied from.
            ins.regs_mut(&mut |v, def| {
                if let (false, Some(&original)) = (def, known.same.get(v)) {
                    *v = original;
                    changed = true;
                }
            });
            let key = match &*ins {
                Ins::Bin(op, d, a, b, w, u) => Some((Key::Bin(*op, *a, *b, *w, *u), *d)),
                Ins::Neg(d, s) => Some((Key::Un(0, *s, 0, 0), *d)),
                Ins::Not(d, s) => Some((Key::Un(1, *s, 0, 0), *d)),
                Ins::FNeg(d, s, size) => Some((Key::Un(2, *s, *size, 0), *d)),
                Ins::Ext(d, s, from, signed) => Some((Key::Un(3, *s, *from, *signed as u8), *d)),
                Ins::F2F(d, s, to) => Some((Key::Un(4, *s, *to, 0), *d)),
                Ins::I2F(d, s, to, kind) => Some((Key::Un(5, *s, *to, *kind as u8), *d)),
                Ins::F2I(d, s, from, kind) => Some((Key::Un(6, *s, *from, *kind as u8), *d)),
                Ins::FBin(op, d, a, b, size) => Some((Key::FBin(*op, *a, *b, *size), *d)),
                Ins::Lea(d, a) => Some((Key::Lea(a.clone()), *d)),
                Ins::GotAddr(d, name) => Some((Key::Sym(false, name.clone()), *d)),
                Ins::TlsAddr(d, name) => Some((Key::Sym(true, name.clone()), *d)),
                Ins::Load(d, a, size, signed, false) => Some((Key::Load(a.clone(), *size, *signed && *size < 4, f.cls[*d] == Cls::Flt), *d)),
                _ => None,
            };
            // The registers the instruction reads, to see whether it also writes one of them.
            let mut reads = vec![];
            ins.regs(&mut |v, def| {
                if !def {
                    reads.push(v);
                }
            });
            // An entry is tied to the registers in it that can be written again.
            let learn = |known: &mut Known, key: Key, value: V, regs: &[V]| {
                for &r in regs.iter().chain([&value]) {
                    if ndef[r] != 1 {
                        known.with.entry(r).or_default().push(key.clone());
                    }
                }
                if matches!(key, Key::Load(..)) { known.loads.insert(key, value) } else { known.pure.insert(key, value) };
            };
            if let Some((key, d)) = key {
                let earlier = if matches!(key, Key::Load(..)) { known.loads.get(&key) } else { known.pure.get(&key) }.copied();
                known.forget(d);
                match earlier {
                    Some(earlier) => {
                        *ins = Ins::Mov(d, earlier);
                        changed = true;
                    }
                    None if !reads.contains(&d) => learn(&mut known, key, d, &reads),
                    None => {}
                }
                continue;
            }
            // A write to memory: what was loaded may no longer be what is there.
            if matches!(ins, Ins::Store(..) | Ins::StoreImm(..) | Ins::Copy(..) | Ins::Zero(..) | Ins::Call(..)) {
                known.loads.clear();
            }
            let mut defs = vec![];
            ins.regs(&mut |v, def| {
                if def {
                    defs.push(v);
                }
            });
            defs.into_iter().for_each(|d| known.forget(d));
            // A temporary copy only: reading a variable through the temporary
            // it was assigned from would keep the temporary alive instead.
            if let Ins::Mov(d, s) = &*ins {
                if d != s && ndef[*d] == 1 {
                    known.same.insert(*d, *s);
                    known.copies.entry(*s).or_default().push(*d);
                }
            }
            if let Ins::Store(v, a, size) = &*ins {
                if *size >= 4 {
                    learn(&mut known, Key::Load(a.clone(), *size, false, f.cls[*v] == Cls::Flt), *v, &reads);
                }
            }
        }
        at_end.insert(b, known);
    }
    changed
}

/// Removes the instructions whose results nothing uses.
fn remove_dead(f: &mut Func) -> bool {
    let (_, live_out) = liveness(f);
    let mut changed = false;
    for i in 0..f.layout.len() {
        let b = f.layout[i];
        let mut live = live_out[b].clone();
        let block = &mut f.blocks[b];
        let mut keep = vec![true; block.len()];
        for (k, ins) in block.iter_mut().enumerate().rev() {
            let mut used = false;
            ins.regs(&mut |v, def| used |= def && live[v / 64] >> (v % 64) & 1 != 0);
            let pure = matches!(
                ins,
                Ins::Imm(..) | Ins::FImm(..) | Ins::Mov(..) | Ins::Bin(..) | Ins::Neg(..) | Ins::Not(..) | Ins::FBin(..) | Ins::FNeg(..) | Ins::Ext(..) | Ins::F2F(..)
                    | Ins::I2F(..) | Ins::F2I(..) | Ins::Load(.., false) | Ins::Lea(..) | Ins::GotAddr(..) | Ins::TlsAddr(..)
            );
            if pure && !used || matches!(ins, Ins::Mov(d, s) if d == s) {
                keep[k] = false;
                changed = true;
                continue;
            }
            match ins {
                // The call stays; its unused results need not be fetched.
                Ins::Call(dsts, ..) if !used && !dsts.is_empty() => {
                    dsts.clear();
                    changed = true;
                }
                Ins::Entry(params) => {
                    let n = params.len();
                    params.retain(|(v, _)| live[v / 64] >> (v % 64) & 1 != 0);
                    changed |= params.len() != n;
                }
                _ => {}
            }
            ins.regs(&mut |v, def| {
                if def {
                    live[v / 64] &= !(1 << (v % 64));
                }
            });
            ins.regs(&mut |v, def| {
                if !def {
                    live[v / 64] |= 1 << (v % 64);
                }
            });
        }
        let mut keep = keep.into_iter();
        block.retain(|_| keep.next().unwrap());
    }
    changed
}

/// Tidies the control flow: jumps to a block that only jumps on go straight
/// to where it goes, blocks that cannot be reached are dropped, and a block
/// joins the one before it if that is its only predecessor.
fn tidy(f: &mut Func) -> bool {
    let nb = f.blocks.len();
    let hop = |mut b: B| {
        for _ in 0..nb {
            match f.blocks[b].as_slice() {
                [Ins::Jmp(t)] if *t != b => b = *t,
                _ => break,
            }
        }
        b
    };
    let to: Vec<B> = (0..nb).map(hop).collect();
    let mut changed = false;
    let mut go = |b: &mut B| {
        changed |= to[*b] != *b;
        *b = to[*b];
    };
    let mut collapsed = false;
    for i in 0..f.layout.len() {
        let term = f.blocks[f.layout[i]].last_mut().unwrap();
        term.targets_mut(&mut go);
        // A branch whose two ways lead to the same block is a jump.
        let same = match term {
            Ins::Br(.., t, e) if t == e => Some(*t),
            _ => None,
        };
        if let Some(t) = same {
            *term = Ins::Jmp(t);
            collapsed = true;
        }
    }
    changed |= collapsed;

    let mut reached = vec![false; nb];
    let mut work = vec![f.layout[0]];
    while let Some(b) = work.pop() {
        if !std::mem::replace(&mut reached[b], true) {
            work.extend(f.blocks[b].last().unwrap().succs());
        }
    }
    changed |= f.layout.iter().any(|b| !reached[*b]);
    f.layout.retain(|b| reached[*b]);

    let mut npred = vec![0; nb];
    for &b in &f.layout {
        let mut succs = f.blocks[b].last().unwrap().succs();
        succs.sort();
        succs.dedup();
        succs.into_iter().for_each(|s| npred[s] += 1);
    }
    let mut gone = vec![false; nb];
    for i in 0..f.layout.len() {
        let b = f.layout[i];
        while let (false, Some(&Ins::Jmp(s))) = (gone[b], f.blocks[b].last()) {
            if s == b || npred[s] != 1 || s == f.layout[0] {
                break;
            }
            f.blocks[b].pop();
            let tail = std::mem::take(&mut f.blocks[s]);
            f.blocks[b].extend(tail);
            gone[s] = true;
            changed = true;
        }
    }
    f.layout.retain(|b| !gone[*b]);
    changed
}
