//! `$((式))` の整数の計算
//!
//! 数（10 進）・変数の名前（値が数でなければ 0）・`+ - * / %`・比較（`< <= > >= == !=`、真は 1）・
//! `&& || !`・単項の `- +`・括弧。`$X` は呼ぶ前に展開しておく。



#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithError {
    Syntax,
    DivByZero,
}

impl core::fmt::Display for ArithError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            ArithError::Syntax => write!(f, "bad arithmetic expression"),
            ArithError::DivByZero => write!(f, "division by zero"),
        }
    }
}

struct P<'a, F: FnMut(&str) -> i64> {
    s: &'a [u8],
    i: usize,
    var: F,
}

type R = Result<i64, ArithError>;

impl<F: FnMut(&str) -> i64> P<'_, F> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    /// 次が `op` なら読む
    fn eat(&mut self, op: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(op.as_bytes()) {
            // "<" と "<=" などを取り違えない
            let next = self.s.get(self.i + op.len()).copied();
            if (op == "<" || op == ">" || op == "!") && next == Some(b'=') {
                return false;
            }
            if (op == "&" || op == "|") && next == Some(op.as_bytes()[0]) {
                return false;
            }
            self.i += op.len();
            return true;
        }
        false
    }

    fn or(&mut self) -> R {
        let mut v = self.and()?;
        while self.eat("||") {
            let r = self.and()?;
            v = (v != 0 || r != 0) as i64;
        }
        Ok(v)
    }

    fn and(&mut self) -> R {
        let mut v = self.eq()?;
        while self.eat("&&") {
            let r = self.eq()?;
            v = (v != 0 && r != 0) as i64;
        }
        Ok(v)
    }

    fn eq(&mut self) -> R {
        let mut v = self.cmp()?;
        loop {
            if self.eat("==") {
                v = (v == self.cmp()?) as i64;
            } else if self.eat("!=") {
                v = (v != self.cmp()?) as i64;
            } else {
                return Ok(v);
            }
        }
    }

    fn cmp(&mut self) -> R {
        let mut v = self.add()?;
        loop {
            if self.eat("<=") {
                v = (v <= self.add()?) as i64;
            } else if self.eat(">=") {
                v = (v >= self.add()?) as i64;
            } else if self.eat("<") {
                v = (v < self.add()?) as i64;
            } else if self.eat(">") {
                v = (v > self.add()?) as i64;
            } else {
                return Ok(v);
            }
        }
    }

    fn add(&mut self) -> R {
        let mut v = self.mul()?;
        loop {
            if self.eat("+") {
                v = v.wrapping_add(self.mul()?);
            } else if self.eat("-") {
                v = v.wrapping_sub(self.mul()?);
            } else {
                return Ok(v);
            }
        }
    }

    fn mul(&mut self) -> R {
        let mut v = self.unary()?;
        loop {
            let op = if self.eat("*") {
                b'*'
            } else if self.eat("/") {
                b'/'
            } else if self.eat("%") {
                b'%'
            } else {
                return Ok(v);
            };
            let r = self.unary()?;
            v = match op {
                b'*' => v.wrapping_mul(r),
                _ if r == 0 => return Err(ArithError::DivByZero),
                b'/' => v.wrapping_div(r),
                _ => v.wrapping_rem(r),
            };
        }
    }

    fn unary(&mut self) -> R {
        if self.eat("-") {
            return Ok(self.unary()?.wrapping_neg());
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok((self.unary()? == 0) as i64);
        }
        self.primary()
    }

    fn primary(&mut self) -> R {
        self.ws();
        if self.eat("(") {
            let v = self.or()?;
            if !self.eat(")") {
                return Err(ArithError::Syntax);
            }
            return Ok(v);
        }
        let start = self.i;
        match self.s.get(self.i) {
            Some(c) if c.is_ascii_digit() => {
                while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
                core::str::from_utf8(&self.s[start..self.i]).unwrap().parse().map_err(|_| ArithError::Syntax)
            }
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
                while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                    self.i += 1;
                }
                let name = core::str::from_utf8(&self.s[start..self.i]).unwrap();
                Ok((self.var)(name))
            }
            _ => Err(ArithError::Syntax),
        }
    }
}

/// 式を計算する。`var` は名前から値（数でなければ 0）
pub fn eval(expr: &str, var: impl FnMut(&str) -> i64) -> Result<i64, ArithError> {
    let mut p = P { s: expr.as_bytes(), i: 0, var };
    p.ws();
    if p.i == p.s.len() {
        return Ok(0);
    }
    let v = p.or()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(ArithError::Syntax);
    }
    Ok(v)
}

/// 変数の値を数として読む（前後の空白は無視、数でなければ 0）
pub fn number(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

