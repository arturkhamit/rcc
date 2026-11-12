use std::cell::RefCell;
use std::rc::Rc;

pub type Ty = Rc<Type>;

pub enum Type {
    Void,
    Bool,
    Int { size: u8, unsigned: bool },
    Float,
    Double,
    LDouble, // x87 80-bit extended precision, 16 bytes in memory
    Ptr(Ty),
    Array(Ty, i64), // len < 0: incomplete
    Vla(Ty, usize), // variable-length array; the local holding its byte size
    Func(Rc<FuncTy>),
    Struct(Rc<RefCell<SDef>>), // structs and unions
}

pub struct FuncTy {
    pub ret: Ty,
    pub params: Vec<Ty>,
    pub names: Vec<String>,
    pub variadic: bool,
    pub oldstyle: bool, // declared as `f()`: unknown parameters
}

pub struct SDef {
    pub is_union: bool,
    pub members: Vec<Member>,
    pub size: i64,
    pub align: i64,
    pub complete: bool,
}

#[derive(Clone)]
pub struct Member {
    pub name: String, // empty: anonymous struct/union member
    pub ty: Ty,
    pub offset: i64,
    pub bit_width: i64, // < 0: not a bitfield
    pub bit_off: i64,
}

impl Type {
    pub fn size(&self) -> i64 {
        match self {
            Type::Void | Type::Bool | Type::Func(_) => 1,
            Type::Int { size, .. } => *size as i64,
            Type::Float => 4,
            Type::Double | Type::Ptr(_) | Type::Vla(..) => 8, // a VLA variable is a pointer to its storage
            Type::LDouble => 16,
            Type::Array(b, n) => if *n < 0 { 0 } else { b.size() * n },
            Type::Struct(s) => s.borrow().size,
        }
    }
    pub fn align(&self) -> i64 {
        match self {
            Type::Array(b, _) => b.align(),
            Type::Struct(s) => s.borrow().align.max(1),
            _ => self.size(),
        }
    }
    pub fn is_integer(&self) -> bool {
        matches!(self, Type::Bool | Type::Int { .. })
    }
    pub fn is_flonum(&self) -> bool {
        matches!(self, Type::Float | Type::Double | Type::LDouble)
    }
    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_flonum()
    }
    pub fn is_struct(&self) -> bool {
        matches!(self, Type::Struct(_))
    }
    /// Unsigned for the purpose of comparison/division/shift (pointers compare unsigned).
    pub fn is_unsigned(&self) -> bool {
        matches!(self, Type::Bool | Type::Int { unsigned: true, .. } | Type::Ptr(_) | Type::Array(..) | Type::Vla(..))
    }
    /// Pointee / element type of pointers and arrays.
    pub fn base(&self) -> Option<&Ty> {
        match self {
            Type::Ptr(b) | Type::Array(b, _) | Type::Vla(b, _) => Some(b),
            _ => None,
        }
    }
}

pub fn ty_void() -> Ty { Rc::new(Type::Void) }
pub fn ty_bool() -> Ty { Rc::new(Type::Bool) }
pub fn ty_ints(size: u8, unsigned: bool) -> Ty { Rc::new(Type::Int { size, unsigned }) }
pub fn ty_char() -> Ty { ty_ints(1, false) }
pub fn ty_int() -> Ty { ty_ints(4, false) }
pub fn ty_uint() -> Ty { ty_ints(4, true) }
pub fn ty_long() -> Ty { ty_ints(8, false) }
pub fn ty_ulong() -> Ty { ty_ints(8, true) }
pub fn ty_float() -> Ty { Rc::new(Type::Float) }
pub fn ty_double() -> Ty { Rc::new(Type::Double) }
pub fn ty_ldouble() -> Ty { Rc::new(Type::LDouble) }
pub fn ptr_to(b: Ty) -> Ty { Rc::new(Type::Ptr(b)) }
pub fn array_of(b: Ty, n: i64) -> Ty { Rc::new(Type::Array(b, n)) }
pub fn new_struct(is_union: bool) -> Ty {
    Rc::new(Type::Struct(Rc::new(RefCell::new(SDef { is_union, members: vec![], size: 0, align: 1, complete: false }))))
}

pub fn align_to(n: i64, a: i64) -> i64 {
    (n + a - 1) / a * a
}

/// Integer promotion.
pub fn promote(t: &Ty) -> Ty {
    match &**t {
        Type::Bool => ty_int(),
        Type::Int { size, .. } if *size < 4 => ty_int(),
        _ => t.clone(),
    }
}

/// Arrays and functions decay to pointers.
pub fn decay(t: &Ty) -> Ty {
    match &**t {
        Type::Array(b, _) | Type::Vla(b, _) => ptr_to(b.clone()),
        Type::Func(_) => ptr_to(t.clone()),
        _ => t.clone(),
    }
}

/// Usual arithmetic conversions.
pub fn common_type(a: &Ty, b: &Ty) -> Ty {
    if a.base().is_some() || matches!(**a, Type::Func(_)) {
        return decay(a);
    }
    if b.base().is_some() || matches!(**b, Type::Func(_)) {
        return decay(b);
    }
    if matches!(**a, Type::LDouble) || matches!(**b, Type::LDouble) {
        return ty_ldouble();
    }
    if matches!(**a, Type::Double) || matches!(**b, Type::Double) {
        return ty_double();
    }
    if matches!(**a, Type::Float) || matches!(**b, Type::Float) {
        return ty_float();
    }
    let (a, b) = (promote(a), promote(b));
    if a.size() != b.size() {
        return if a.size() < b.size() { b } else { a };
    }
    if b.is_unsigned() { b } else { a }
}

/// Finds a member by name, looking through anonymous struct/union members.
/// Returns the chain of members leading to it.
pub fn find_member(ty: &Ty, name: &str) -> Option<Vec<Member>> {
    let Type::Struct(sd) = &**ty else { return None };
    for m in &sd.borrow().members {
        if m.name == name {
            return Some(vec![m.clone()]);
        }
        if m.name.is_empty() {
            if let Some(mut path) = find_member(&m.ty, name) {
                path.insert(0, m.clone());
                return Some(path);
            }
        }
    }
    None
}

/// Type compatibility for `_Generic` and `__builtin_types_compatible_p`.
pub fn same_type(a: &Ty, b: &Ty) -> bool {
    match (&**a, &**b) {
        (Type::Void, Type::Void) | (Type::Bool, Type::Bool) | (Type::Float, Type::Float) | (Type::Double, Type::Double) | (Type::LDouble, Type::LDouble) => true,
        (Type::Int { size: s1, unsigned: u1 }, Type::Int { size: s2, unsigned: u2 }) => s1 == s2 && u1 == u2,
        (Type::Ptr(x), Type::Ptr(y)) => same_type(x, y),
        (Type::Array(x, n), Type::Array(y, m)) => (n == m || *n < 0 || *m < 0) && same_type(x, y),
        (Type::Func(f), Type::Func(g)) => {
            same_type(&f.ret, &g.ret)
                && (f.oldstyle || g.oldstyle || f.variadic == g.variadic && f.params.len() == g.params.len() && f.params.iter().zip(&g.params).all(|(x, y)| same_type(x, y)))
        }
        (Type::Struct(x), Type::Struct(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

/// True if the type contains a `long double` (such structs are passed in memory).
pub fn has_ldouble(ty: &Ty) -> bool {
    match &**ty {
        Type::LDouble => true,
        Type::Array(b, _) => has_ldouble(b),
        Type::Struct(s) => s.borrow().members.iter().any(|m| has_ldouble(&m.ty)),
        _ => false,
    }
}

/// Converts a double to the x87 extended format: (mantissa, sign and exponent).
pub fn f64_to_f80(v: f64) -> (u64, u16) {
    let bits = v.to_bits();
    let sign = ((bits >> 63) as u16) << 15;
    let (exp, frac) = (((bits >> 52) & 0x7ff) as i32, bits & ((1 << 52) - 1));
    match (exp, frac) {
        (0, 0) => (0, sign),
        (0x7ff, 0) => (1 << 63, sign | 0x7fff),
        (0x7ff, _) => (3 << 62 | frac << 11, sign | 0x7fff),
        (0, _) => {
            let lz = frac.leading_zeros() as i32;
            (frac << lz, sign | (16383 - 1074 + 63 - lz) as u16)
        }
        _ => (1 << 63 | frac << 11, sign | (exp - 1023 + 16383) as u16),
    }
}

/// Approximates an x87 extended value by a double.
pub fn f80_to_f64(m: u64, se: u16) -> f64 {
    let e = (se & 0x7fff) as i32;
    let v = if e == 0x7fff {
        if m << 1 == 0 { f64::INFINITY } else { f64::NAN }
    } else {
        let e = e.max(1) - 16383 - 63;
        m as f64 * 2f64.powi(e / 2) * 2f64.powi(e - e / 2)
    };
    if se >> 15 != 0 { -v } else { v }
}
