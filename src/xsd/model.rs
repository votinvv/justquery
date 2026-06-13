//! Модель скомпилированной XSD-схемы и проверка значений.
//!
//! Подмножество XSD, реально используемое схемами: sequence/choice c
//! квантификаторами {1, ?, *, +}, complexContent/extension, простые типы с фасетами
//! enumeration/pattern/length/minLength/maxLength/minInclusive/maxInclusive/totalDigits,
//! union по ссылкам, атрибуты use="required|optional". Неймспейсы не используются.
//!
//! Контентные модели компилируются в ε-NFA по Томпсону; XSD требует детерминизма
//! (UPA), поэтому прогон множеством состояний однозначен.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeRef {
    Simple(usize),
    Complex(usize),
    /// Тип не задан (xs:anyType) — содержимое не проверяется.
    Any,
}

// ---------------------------------------------------------------------------
//  Простые типы
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Builtin {
    String,
    Date,
    DateTime,
    Time,
    Decimal,
    UnsignedLong,
    UnsignedInt,
    UnsignedShort,
    PositiveInteger,
}

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "string" => Builtin::String,
            "date" => Builtin::Date,
            "dateTime" => Builtin::DateTime,
            "time" => Builtin::Time,
            "decimal" => Builtin::Decimal,
            "unsignedLong" => Builtin::UnsignedLong,
            "unsignedInt" => Builtin::UnsignedInt,
            "unsignedShort" => Builtin::UnsignedShort,
            "positiveInteger" => Builtin::PositiveInteger,
            _ => return None,
        })
    }

    /// Проверить лексическое значение встроенного типа.
    pub fn check(self, v: &str) -> Result<(), String> {
        match self {
            Builtin::String => Ok(()),
            Builtin::Date => check_date(v),
            Builtin::DateTime => check_datetime(v),
            Builtin::Time => check_time(v, true),
            Builtin::Decimal => check_decimal(v),
            Builtin::UnsignedLong => check_unsigned(v, u64::MAX as u128),
            Builtin::UnsignedInt => check_unsigned(v, u32::MAX as u128),
            Builtin::UnsignedShort => check_unsigned(v, u16::MAX as u128),
            Builtin::PositiveInteger => {
                check_unsigned(v, u128::MAX)?;
                if v.trim_start_matches('0').is_empty() {
                    return Err("ожидается положительное целое число".to_owned());
                }
                Ok(())
            }
        }
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn check_unsigned(v: &str, max: u128) -> Result<(), String> {
    if !all_digits(v) {
        return Err("ожидается неотрицательное целое число".to_owned());
    }
    match v.parse::<u128>() {
        Ok(n) if n <= max => Ok(()),
        _ => Err("число выходит за допустимый диапазон".to_owned()),
    }
}

fn check_decimal(v: &str) -> Result<(), String> {
    let s = v.strip_prefix(['+', '-']).unwrap_or(v);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("ожидается десятичное число".to_owned());
    }
    Ok(())
}

/// Дата ГГГГ-ММ-ДД с необязательной зоной (Z | ±чч:мм).
fn check_date(v: &str) -> Result<(), String> {
    if !v.is_ascii() {
        return Err("ожидается дата в формате ГГГГ-ММ-ДД".to_owned());
    }
    let (date, rest) = v.split_at(v.len().min(10));
    check_tz(rest)?;
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return Err("ожидается дата в формате ГГГГ-ММ-ДД".to_owned());
    }
    let y: u32 = date[0..4].parse().map_err(|_| "некорректный год".to_owned())?;
    let m: u32 = date[5..7].parse().map_err(|_| "некорректный месяц".to_owned())?;
    let d: u32 = date[8..10].parse().map_err(|_| "некорректный день".to_owned())?;
    if !(1..=12).contains(&m) {
        return Err("месяц вне диапазона 01–12".to_owned());
    }
    if d < 1 || d > days_in_month(y, m) {
        return Err("день вне диапазона для данного месяца".to_owned());
    }
    Ok(())
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn check_tz(rest: &str) -> Result<(), String> {
    if rest.is_empty() || rest == "Z" {
        return Ok(());
    }
    let b = rest.as_bytes();
    if b.len() == 6
        && (b[0] == b'+' || b[0] == b'-')
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3] == b':'
        && b[4].is_ascii_digit()
        && b[5].is_ascii_digit()
    {
        return Ok(());
    }
    Err("некорректная зона времени".to_owned())
}

fn check_time(v: &str, allow_tz: bool) -> Result<(), String> {
    if !v.is_ascii() {
        return Err("ожидается время в формате чч:мм:сс".to_owned());
    }
    let (t, rest) = v.split_at(v.len().min(8));
    let b = t.as_bytes();
    if b.len() != 8 || b[2] != b':' || b[5] != b':' {
        return Err("ожидается время в формате чч:мм:сс".to_owned());
    }
    let h: u32 = t[0..2].parse().map_err(|_| "некорректные часы".to_owned())?;
    let m: u32 = t[3..5].parse().map_err(|_| "некорректные минуты".to_owned())?;
    let s: u32 = t[6..8].parse().map_err(|_| "некорректные секунды".to_owned())?;
    if h > 23 || m > 59 || s > 59 {
        return Err("время вне диапазона".to_owned());
    }
    // дробные секунды и зона
    let mut rest = rest;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: usize = after_dot.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return Err("некорректные дробные секунды".to_owned());
        }
        rest = &after_dot[digits..];
    }
    if allow_tz {
        check_tz(rest)
    } else if rest.is_empty() {
        Ok(())
    } else {
        Err("лишние символы после времени".to_owned())
    }
}

/// Дата-время ГГГГ-ММ-ДДTчч:мм:сс(.доли)?(зона)?.
fn check_datetime(v: &str) -> Result<(), String> {
    let Some((d, t)) = v.split_once('T') else {
        return Err("ожидается дата-время в формате ГГГГ-ММ-ДДTчч:мм:сс".to_owned());
    };
    // дату проверяем без зоны (зона относится ко всему значению)
    let db = d.as_bytes();
    if db.len() != 10 {
        return Err("ожидается дата в формате ГГГГ-ММ-ДД".to_owned());
    }
    check_date(d)?;
    check_time(t, true)
}

/// Один шаг цепочки restriction: все фасеты шага должны выполняться.
#[derive(Default)]
pub struct Facets {
    pub enums: Option<Vec<String>>,
    pub patterns: Vec<regex::Regex>,
    pub length: Option<usize>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub min_inclusive: Option<f64>,
    pub max_inclusive: Option<f64>,
    pub total_digits: Option<u32>,
}

impl Facets {
    pub fn is_empty(&self) -> bool {
        self.enums.is_none()
            && self.patterns.is_empty()
            && self.length.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.min_inclusive.is_none()
            && self.max_inclusive.is_none()
            && self.total_digits.is_none()
    }

    fn check(&self, v: &str) -> Result<(), String> {
        if let Some(en) = &self.enums {
            if !en.iter().any(|e| e == v) {
                return Err(format!(
                    "значение «{}» не входит в список допустимых",
                    clip(v)
                ));
            }
        }
        if !self.patterns.is_empty() && !self.patterns.iter().any(|p| p.is_match(v)) {
            return Err(format!("значение «{}» не соответствует требуемому формату", clip(v)));
        }
        let n_chars = || v.chars().count();
        if let Some(l) = self.length {
            if n_chars() != l {
                return Err(format!("длина должна быть ровно {l} симв."));
            }
        }
        if let Some(l) = self.min_length {
            if n_chars() < l {
                return Err(format!("длина меньше минимальной ({l} симв.)"));
            }
        }
        if let Some(l) = self.max_length {
            if n_chars() > l {
                return Err(format!("длина больше максимальной ({l} симв.)"));
            }
        }
        if self.min_inclusive.is_some() || self.max_inclusive.is_some() {
            let num: f64 = v.parse().map_err(|_| "ожидается число".to_owned())?;
            if let Some(mn) = self.min_inclusive {
                if num < mn {
                    return Err(format!("значение меньше минимального ({mn})"));
                }
            }
            if let Some(mx) = self.max_inclusive {
                if num > mx {
                    return Err(format!("значение больше максимального ({mx})"));
                }
            }
        }
        if let Some(td) = self.total_digits {
            let digits = v.bytes().filter(|b| b.is_ascii_digit()).count() as u32;
            if digits > td {
                return Err(format!("число цифр больше допустимого ({td})"));
            }
        }
        Ok(())
    }
}

fn clip(v: &str) -> String {
    if v.chars().count() > 60 {
        let cut: String = v.chars().take(57).collect();
        format!("{cut}…")
    } else {
        v.to_owned()
    }
}

pub enum Variety {
    Atomic { base: Builtin, steps: Vec<Facets> },
    Union { members: Vec<usize> },
}

pub struct SimpleType {
    #[allow(dead_code)] // для диагностики; у анонимных пусто
    pub name: String,
    pub variety: Variety,
}

// ---------------------------------------------------------------------------
//  Контентные модели (частицы → NFA)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quant {
    One,       // [1,1]
    Opt,       // [0,1]
    Star,      // [0,∞]
    Plus,      // [1,∞]
}

#[derive(Clone, Debug)]
pub enum Particle {
    Elem { name: String, ty: TypeRef, q: Quant },
    Seq { items: Vec<Particle>, q: Quant },
    Choice { items: Vec<Particle>, q: Quant },
}

/// ε-NFA контентной модели. Переход несёт тип ребёнка для спуска.
pub struct Nfa {
    pub eps: Vec<Vec<usize>>,
    pub trans: Vec<Vec<(String, usize, TypeRef)>>,
    pub start: usize,
    pub accept: usize,
}

impl Nfa {
    fn new_state(&mut self) -> usize {
        self.eps.push(Vec::new());
        self.trans.push(Vec::new());
        self.eps.len() - 1
    }

    pub fn build(p: &Particle) -> Nfa {
        let mut nfa = Nfa { eps: Vec::new(), trans: Vec::new(), start: 0, accept: 0 };
        let s = nfa.new_state();
        let a = nfa.new_state();
        nfa.start = s;
        nfa.accept = a;
        nfa.frag(p, s, a);
        nfa
    }

    /// Построить фрагмент частицы между состояниями `from` и `to`.
    fn frag(&mut self, p: &Particle, from: usize, to: usize) {
        let q = match p {
            Particle::Elem { q, .. } | Particle::Seq { q, .. } | Particle::Choice { q, .. } => *q,
        };
        // обёртка квантификатора: внутренние i→o, плюс обходы/петли
        let i = self.new_state();
        let o = self.new_state();
        self.eps[from].push(i);
        self.eps[o].push(to);
        match q {
            Quant::One => {}
            Quant::Opt => self.eps[from].push(to),
            Quant::Star => {
                self.eps[from].push(to);
                self.eps[o].push(i);
            }
            Quant::Plus => self.eps[o].push(i),
        }
        match p {
            Particle::Elem { name, ty, .. } => {
                self.trans[i].push((name.clone(), o, *ty));
            }
            Particle::Seq { items, .. } => {
                let mut cur = i;
                for (k, item) in items.iter().enumerate() {
                    let next = if k + 1 == items.len() { o } else { self.new_state() };
                    self.frag(item, cur, next);
                    cur = next;
                }
                if items.is_empty() {
                    self.eps[i].push(o);
                }
            }
            Particle::Choice { items, .. } => {
                if items.is_empty() {
                    self.eps[i].push(o);
                }
                for item in items {
                    self.frag(item, i, o);
                }
            }
        }
    }

    /// ε-замыкание множества состояний (in-place, сортировка/дедуп).
    pub fn closure(&self, set: &mut Vec<usize>) {
        let mut i = 0;
        while i < set.len() {
            let s = set[i];
            for &e in &self.eps[s] {
                if !set.contains(&e) {
                    set.push(e);
                }
            }
            i += 1;
        }
        set.sort_unstable();
        set.dedup();
    }

    /// Начальное множество состояний.
    pub fn start_set(&self) -> Vec<usize> {
        let mut s = vec![self.start];
        self.closure(&mut s);
        s
    }

    /// Продвинуться по имени ребёнка. Ok((новое множество, тип ребёнка)) или Err(ожидаемые).
    pub fn advance(&self, set: &[usize], name: &str) -> Result<(Vec<usize>, TypeRef), Vec<String>> {
        let mut out = Vec::new();
        let mut ty = None;
        for &s in set {
            for (label, to, t) in &self.trans[s] {
                if label == name {
                    out.push(*to);
                    ty.get_or_insert(*t);
                }
            }
        }
        if out.is_empty() {
            return Err(self.expected(set));
        }
        self.closure(&mut out);
        Ok((out, ty.unwrap()))
    }

    /// Допустимо ли завершиться из текущего множества.
    pub fn accepting(&self, set: &[usize]) -> bool {
        set.contains(&self.accept)
    }

    /// Имена, допустимые из текущего множества (для сообщений об ошибках).
    pub fn expected(&self, set: &[usize]) -> Vec<String> {
        let mut names: Vec<String> = set
            .iter()
            .flat_map(|&s| self.trans[s].iter().map(|(n, _, _)| n.clone()))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

// ---------------------------------------------------------------------------
//  Комплексные типы и схема
// ---------------------------------------------------------------------------

pub struct AttrDecl {
    pub name: String,
    pub required: bool,
    pub ty: Option<usize>, // индекс простого типа; None — без проверки
}

pub struct ComplexType {
    #[allow(dead_code)] // для диагностики; у анонимных пусто
    pub name: String,
    pub particle: Option<Particle>,
    pub nfa: Option<Nfa>,
    pub attrs: Vec<AttrDecl>,
}

pub struct Schema {
    #[allow(dead_code)] // для диагностики
    pub version: String,
    pub simples: Vec<SimpleType>,
    pub complexes: Vec<ComplexType>,
    pub root_name: String,
    pub root_type: TypeRef,
}

impl Schema {
    /// Проверить значение простого типа `id`.
    pub fn check_simple(&self, id: usize, v: &str) -> Result<(), String> {
        match &self.simples[id].variety {
            Variety::Atomic { base, steps } => {
                base.check(v)?;
                for st in steps {
                    st.check(v)?;
                }
                Ok(())
            }
            Variety::Union { members } => {
                for &m in members {
                    if self.check_simple(m, v).is_ok() {
                        return Ok(());
                    }
                }
                Err(format!(
                    "значение «{}» не соответствует ни одному из допустимых форматов",
                    clip(v)
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_date() {
        assert!(check_date("2024-02-29").is_ok());
        assert!(check_date("2023-02-29").is_err());
        assert!(check_date("2023-13-01").is_err());
        assert!(check_date("2023-1-01").is_err());
        assert!(check_date("2023-01-01Z").is_ok());
        assert!(check_date("2023-01-01+03:00").is_ok());
    }

    #[test]
    fn builtin_datetime_time() {
        assert!(check_datetime("2023-01-01T10:20:30").is_ok());
        assert!(check_datetime("2023-01-01T10:20:30.123+03:00").is_ok());
        assert!(check_datetime("2023-01-01 10:20:30").is_err());
        assert!(check_time("23:59:59", true).is_ok());
        assert!(check_time("24:00:00", true).is_err());
    }

    #[test]
    fn builtin_numbers() {
        assert!(Builtin::UnsignedShort.check("65535").is_ok());
        assert!(Builtin::UnsignedShort.check("65536").is_err());
        assert!(Builtin::Decimal.check("123.45").is_ok());
        assert!(Builtin::Decimal.check("12a").is_err());
        assert!(Builtin::PositiveInteger.check("0").is_err());
        assert!(Builtin::PositiveInteger.check("1").is_ok());
    }

    fn elem(name: &str, q: Quant) -> Particle {
        Particle::Elem { name: name.to_owned(), ty: TypeRef::Any, q }
    }

    #[test]
    fn nfa_sequence_with_optionals() {
        // a, b?, c
        let p = Particle::Seq {
            items: vec![elem("a", Quant::One), elem("b", Quant::Opt), elem("c", Quant::One)],
            q: Quant::One,
        };
        let nfa = Nfa::build(&p);
        let s0 = nfa.start_set();
        assert!(!nfa.accepting(&s0));
        let (s1, _) = nfa.advance(&s0, "a").unwrap();
        let (s2, _) = nfa.advance(&s1, "c").unwrap(); // b пропущен
        assert!(nfa.accepting(&s2));
        let (s1b, _) = nfa.advance(&s0, "a").unwrap();
        let (s2b, _) = nfa.advance(&s1b, "b").unwrap();
        assert!(!nfa.accepting(&s2b));
        let (s3b, _) = nfa.advance(&s2b, "c").unwrap();
        assert!(nfa.accepting(&s3b));
        // неожиданный элемент
        assert!(nfa.advance(&s0, "x").is_err());
    }

    #[test]
    fn nfa_choice_unbounded() {
        // (a | b)+
        let p = Particle::Choice {
            items: vec![elem("a", Quant::One), elem("b", Quant::One)],
            q: Quant::Plus,
        };
        let nfa = Nfa::build(&p);
        let s0 = nfa.start_set();
        assert!(!nfa.accepting(&s0));
        let (s1, _) = nfa.advance(&s0, "b").unwrap();
        assert!(nfa.accepting(&s1));
        let (s2, _) = nfa.advance(&s1, "a").unwrap();
        assert!(nfa.accepting(&s2));
    }

    #[test]
    fn nfa_expected_names() {
        let p = Particle::Seq {
            items: vec![elem("a", Quant::One), elem("b", Quant::One)],
            q: Quant::One,
        };
        let nfa = Nfa::build(&p);
        let s0 = nfa.start_set();
        assert_eq!(nfa.expected(&s0), vec!["a".to_owned()]);
        let err = nfa.advance(&s0, "b").unwrap_err();
        assert_eq!(err, vec!["a".to_owned()]);
    }

    #[test]
    fn facet_checks() {
        let f = Facets {
            enums: Some(vec!["A".into(), "B".into()]),
            ..Default::default()
        };
        assert!(f.check("A").is_ok());
        assert!(f.check("C").is_err());
        let f = Facets {
            patterns: vec![regex::Regex::new(r"\A(?:\d{3})\z").unwrap()],
            ..Default::default()
        };
        assert!(f.check("123").is_ok());
        assert!(f.check("12").is_err());
        assert!(f.check("1234").is_err());
    }
}
