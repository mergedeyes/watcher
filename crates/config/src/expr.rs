//! `apply_to` expressions, e.g. `host.tags contains "linux" and not host.name ~ "^test-"`.
//!
//! ```text
//! expr    := and ( ("or" | "||") and )*
//! and     := unary ( ("and" | "&&") unary )*
//! unary   := ("not" | "!") unary | "(" expr ")" | "true" | "false" | compare
//! compare := operand op operand
//! op      := "==" | "!=" | "~" | "!~" | "contains" | "in"
//! operand := path | "string" | 'string' | [ "list", ... ]
//! path    := host.name | host.address | host.node | host.tags | host.vars.<name>
//! ```

use crate::Host;
use regex::Regex;
use std::fmt;

#[derive(Debug, Clone)]
pub struct Expr(Node);

#[derive(Debug, Clone)]
enum Node {
    Const(bool),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Eq(Operand, Operand),
    Ne(Operand, Operand),
    Match(Operand, Regex),
    NotMatch(Operand, Regex),
    /// Left list (or string) contains right string.
    Contains(Operand, Operand),
}

#[derive(Debug, Clone)]
enum Operand {
    Path(Path),
    Str(String),
    List(Vec<String>),
}

#[derive(Debug, Clone)]
enum Path {
    Name,
    Address,
    Node,
    Tags,
    Var(String),
}

/// A value an operand resolves to for a given host.
enum Value<'a> {
    Str(&'a str),
    List(&'a [String]),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at column {})", self.msg, self.pos + 1)
    }
}

impl std::error::Error for ExprError {}

impl Expr {
    pub fn parse(src: &str) -> Result<Self, ExprError> {
        let tokens = tokenize(src)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            end: src.len(),
        };
        let node = parser.or()?;
        match parser.peek() {
            None => Ok(Expr(node)),
            Some(t) => Err(parser.error_at(t.pos, format!("unexpected {}", t.kind))),
        }
    }

    pub fn matches(&self, host: &Host) -> bool {
        eval(&self.0, host)
    }
}

fn eval(node: &Node, host: &Host) -> bool {
    match node {
        Node::Const(b) => *b,
        Node::Not(n) => !eval(n, host),
        Node::And(a, b) => eval(a, host) && eval(b, host),
        Node::Or(a, b) => eval(a, host) || eval(b, host),
        Node::Eq(a, b) => equals(&resolve(a, host), &resolve(b, host)),
        Node::Ne(a, b) => !equals(&resolve(a, host), &resolve(b, host)),
        Node::Match(a, re) => matches(&resolve(a, host), re),
        Node::NotMatch(a, re) => !matches(&resolve(a, host), re),
        Node::Contains(a, b) => match (resolve(a, host), resolve(b, host)) {
            (Value::List(list), Value::Str(s)) => list.iter().any(|x| x == s),
            (Value::Str(hay), Value::Str(needle)) => hay.contains(needle),
            _ => false,
        },
    }
}

fn resolve<'a>(op: &'a Operand, host: &'a Host) -> Value<'a> {
    match op {
        Operand::Str(s) => Value::Str(s),
        Operand::List(l) => Value::List(l),
        Operand::Path(Path::Name) => Value::Str(&host.name),
        Operand::Path(Path::Address) => Value::Str(&host.address),
        Operand::Path(Path::Node) => host.node.as_deref().map_or(Value::Missing, Value::Str),
        Operand::Path(Path::Tags) => Value::List(&host.tags),
        Operand::Path(Path::Var(name)) => host
            .vars
            .get(name)
            .map_or(Value::Missing, |v| Value::Str(v)),
    }
}

fn equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::List(a), Value::List(b)) => a == b,
        _ => false,
    }
}

/// A list matches when any element does; a missing value never matches.
fn matches(v: &Value, re: &Regex) -> bool {
    match v {
        Value::Str(s) => re.is_match(s),
        Value::List(l) => l.iter().any(|s| re.is_match(s)),
        Value::Missing => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Ident(String),
    Str(String),
    Dot,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    EqEq,
    Ne,
    Tilde,
    NotTilde,
    Bang,
    AndAnd,
    OrOr,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Ident(s) => write!(f, "{s:?}"),
            Kind::Str(s) => write!(f, "string {s:?}"),
            Kind::Dot => f.write_str("'.'"),
            Kind::Comma => f.write_str("','"),
            Kind::LParen => f.write_str("'('"),
            Kind::RParen => f.write_str("')'"),
            Kind::LBracket => f.write_str("'['"),
            Kind::RBracket => f.write_str("']'"),
            Kind::EqEq => f.write_str("'=='"),
            Kind::Ne => f.write_str("'!='"),
            Kind::Tilde => f.write_str("'~'"),
            Kind::NotTilde => f.write_str("'!~'"),
            Kind::Bang => f.write_str("'!'"),
            Kind::AndAnd => f.write_str("'&&'"),
            Kind::OrOr => f.write_str("'||'"),
        }
    }
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    pos: usize,
}

fn tokenize(src: &str) -> Result<Vec<Token>, ExprError> {
    let mut tokens = Vec::new();
    let mut chars = src.char_indices().peekable();
    while let Some(&(pos, c)) = chars.peek() {
        let err = |msg: &str| ExprError {
            pos,
            msg: msg.into(),
        };
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let kind = if c.is_alphabetic() || c == '_' {
            let mut ident = String::new();
            while let Some(&(_, c)) = chars.peek() {
                if !(c.is_alphanumeric() || c == '_' || c == '-') {
                    break;
                }
                ident.push(c);
                chars.next();
            }
            tokens.push(Token {
                kind: Kind::Ident(ident),
                pos,
            });
            continue;
        } else if c == '"' || c == '\'' {
            chars.next();
            let mut s = String::new();
            loop {
                match chars.next() {
                    None => return Err(err("unterminated string")),
                    Some((_, q)) if q == c => break,
                    Some((_, '\\')) => match chars.next() {
                        Some((_, e)) => s.push(e),
                        None => return Err(err("unterminated string")),
                    },
                    Some((_, ch)) => s.push(ch),
                }
            }
            tokens.push(Token {
                kind: Kind::Str(s),
                pos,
            });
            continue;
        } else {
            chars.next();
            let next = chars.peek().map(|&(_, c)| c);
            let mut two = |kind| {
                chars.next();
                kind
            };
            match (c, next) {
                ('=', Some('=')) => two(Kind::EqEq),
                ('!', Some('=')) => two(Kind::Ne),
                ('!', Some('~')) => two(Kind::NotTilde),
                ('&', Some('&')) => two(Kind::AndAnd),
                ('|', Some('|')) => two(Kind::OrOr),
                ('!', _) => Kind::Bang,
                ('~', _) => Kind::Tilde,
                ('.', _) => Kind::Dot,
                (',', _) => Kind::Comma,
                ('(', _) => Kind::LParen,
                (')', _) => Kind::RParen,
                ('[', _) => Kind::LBracket,
                (']', _) => Kind::RBracket,
                ('=', _) => return Err(err("unexpected '=' (use '==')")),
                _ => return Err(err(&format!("unexpected character {c:?}"))),
            }
        };
        tokens.push(Token { kind, pos });
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn error_at(&self, pos: usize, msg: String) -> ExprError {
        ExprError { pos, msg }
    }

    fn here(&self) -> usize {
        self.peek().map_or(self.end, |t| t.pos)
    }

    /// Consumes the next token if it is `kind` or the keyword `word`.
    fn eat(&mut self, kind: Kind, word: &str) -> bool {
        match self.peek().map(|t| &t.kind) {
            Some(k) if *k == kind => {}
            Some(Kind::Ident(w)) if w == word => {}
            _ => return false,
        }
        self.pos += 1;
        true
    }

    fn or(&mut self) -> Result<Node, ExprError> {
        let mut node = self.and()?;
        while self.eat(Kind::OrOr, "or") {
            node = Node::Or(Box::new(node), Box::new(self.and()?));
        }
        Ok(node)
    }

    fn and(&mut self) -> Result<Node, ExprError> {
        let mut node = self.unary()?;
        while self.eat(Kind::AndAnd, "and") {
            node = Node::And(Box::new(node), Box::new(self.unary()?));
        }
        Ok(node)
    }

    fn unary(&mut self) -> Result<Node, ExprError> {
        if self.eat(Kind::Bang, "not") {
            return Ok(Node::Not(Box::new(self.unary()?)));
        }
        if self.eat(Kind::LParen, "") {
            let node = self.or()?;
            if !self.eat(Kind::RParen, "") {
                return Err(self.error_at(self.here(), "expected ')'".into()));
            }
            return Ok(node);
        }
        if self.eat(Kind::Ident("true".into()), "true") {
            return Ok(Node::Const(true));
        }
        if self.eat(Kind::Ident("false".into()), "false") {
            return Ok(Node::Const(false));
        }
        self.compare()
    }

    fn compare(&mut self) -> Result<Node, ExprError> {
        let left = self.operand()?;
        let op_pos = self.here();
        let Some(op) = self.next() else {
            return Err(self.error_at(
                op_pos,
                "expected an operator (==, !=, ~, !~, contains, in)".into(),
            ));
        };
        let regex = |p: &mut Self| -> Result<Regex, ExprError> {
            let pos = p.here();
            match p.operand()? {
                Operand::Str(s) => {
                    Regex::new(&s).map_err(|e| p.error_at(pos, format!("invalid regex: {e}")))
                }
                _ => Err(p.error_at(pos, "expected a regex string after '~'".into())),
            }
        };
        Ok(match op.kind {
            Kind::EqEq => Node::Eq(left, self.operand()?),
            Kind::Ne => Node::Ne(left, self.operand()?),
            Kind::Tilde => Node::Match(left, regex(self)?),
            Kind::NotTilde => Node::NotMatch(left, regex(self)?),
            Kind::Ident(w) if w == "contains" => Node::Contains(left, self.operand()?),
            Kind::Ident(w) if w == "in" => Node::Contains(self.operand()?, left),
            other => {
                return Err(self.error_at(
                    op.pos,
                    format!("expected an operator (==, !=, ~, !~, contains, in), found {other}"),
                ));
            }
        })
    }

    fn operand(&mut self) -> Result<Operand, ExprError> {
        let pos = self.here();
        match self.next().map(|t| t.kind) {
            Some(Kind::Str(s)) => Ok(Operand::Str(s)),
            Some(Kind::LBracket) => {
                let mut items = Vec::new();
                loop {
                    if self.eat(Kind::RBracket, "") {
                        break;
                    }
                    let item_pos = self.here();
                    match self.next().map(|t| t.kind) {
                        Some(Kind::Str(s)) => items.push(s),
                        _ => {
                            return Err(self.error_at(item_pos, "expected a string in list".into()));
                        }
                    }
                    if !self.eat(Kind::Comma, "")
                        && !matches!(self.peek().map(|t| &t.kind), Some(Kind::RBracket))
                    {
                        return Err(self.error_at(self.here(), "expected ',' or ']'".into()));
                    }
                }
                Ok(Operand::List(items))
            }
            Some(Kind::Ident(root)) => {
                let mut parts = vec![root];
                while self.eat(Kind::Dot, "") {
                    match self.next().map(|t| t.kind) {
                        Some(Kind::Ident(p)) => parts.push(p),
                        _ => {
                            return Err(
                                self.error_at(self.here(), "expected a name after '.'".into())
                            );
                        }
                    }
                }
                let path = match parts
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    ["host", "name"] => Path::Name,
                    ["host", "address"] => Path::Address,
                    ["host", "node"] => Path::Node,
                    ["host", "tags"] => Path::Tags,
                    ["host", "vars", var] => Path::Var(var.to_string()),
                    _ => {
                        return Err(self.error_at(
                            pos,
                            format!(
                                "unknown field {:?} (expected host.name, host.address, host.node, host.tags or host.vars.<name>)",
                                parts.join(".")
                            ),
                        ));
                    }
                };
                Ok(Operand::Path(path))
            }
            Some(other) => Err(self.error_at(pos, format!("expected a value, found {other}"))),
            None => Err(self.error_at(pos, "unexpected end of expression".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn host(name: &str, tags: &[&str]) -> Host {
        Host {
            name: name.into(),
            address: format!("{name}.example"),
            node: None,
            check: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            vars: BTreeMap::from([("os".to_string(), "debian".to_string())]),
            enabled: true,
            origin: Default::default(),
        }
    }

    fn check(src: &str, h: &Host) -> bool {
        Expr::parse(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"))
            .matches(h)
    }

    #[test]
    fn evaluates_expressions() {
        let web = host("web01", &["linux", "web"]);
        assert!(check(r#"host.tags contains "linux""#, &web));
        assert!(check(r#""web" in host.tags"#, &web));
        assert!(check(r#"host.name ~ "^web""#, &web));
        assert!(check(r#"host.name !~ "^mq-""#, &web));
        assert!(check(
            r#"host.vars.os == "debian" && host.address == 'web01.example'"#,
            &web
        ));
        assert!(check(r#"host.name in ["db01", "web01"]"#, &web));
        assert!(check(r#"not host.tags contains "windows""#, &web));
        assert!(check(
            r#"host.tags contains "db" or (host.tags contains "web" and !host.name == "x")"#,
            &web
        ));
        assert!(check("true", &web));
        assert!(!check(r#"host.tags contains "db""#, &web));
        assert!(!check(r#"host.vars.missing == "x""#, &web));
        assert!(check(r#"host.vars.missing != "x""#, &web));
        assert!(!check(r#"host.node ~ ".""#, &web));
        assert!(check(r#"host.tags ~ "^we""#, &web));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let h = host("a", &[]);
        assert!(check(r#"true or false and false"#, &h));
        assert!(!check(r#"(true or false) and false"#, &h));
    }

    #[test]
    fn reports_errors_with_position() {
        let err = |src: &str| Expr::parse(src).unwrap_err();
        assert_eq!(err(r#"host.nme == "x""#).pos, 0);
        assert!(err(r#"host.nme == "x""#).msg.contains("unknown field"));
        assert_eq!(err(r#"host.name = "x""#).pos, 10);
        assert!(err(r#"host.name ~ "(""#).msg.contains("invalid regex"));
        assert!(
            err(r#"host.name ~ host.address"#)
                .msg
                .contains("regex string")
        );
        assert!(
            err(r#"host.name == "x" and"#)
                .msg
                .contains("end of expression")
        );
        assert!(err(r#"(host.name == "x""#).msg.contains("')'"));
        assert!(err(r#"host.name"#).msg.contains("operator"));
        assert!(err(r#"host.name == "x" "y""#).msg.contains("unexpected"));
        assert!(err(r#"host.name == "x"#).msg.contains("unterminated"));
    }
}
