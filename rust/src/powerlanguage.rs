//! Deliberately bounded PowerLanguage interpreter. Unsupported syntax is an error.
use crate::backtest::Candle;
use chrono::{Datelike, NaiveDate};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
};

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: usize,
    pub node: Node,
}
#[derive(Clone, Debug)]
pub enum Node {
    Number(f64),
    Text(String),
    Name(String),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    History(Box<Expr>, Box<Expr>),
    ArrayIndex(String, Box<Expr>),
}
#[derive(Clone, Debug)]
pub enum Stmt {
    SetExactComparison,
    SetMediumComparison,
    Assign(String, Expr),
    ArrayAssign(String, Expr, Expr),
    For(String, Expr, Expr, bool, Vec<Stmt>),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Order(OrderSpec),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Buy,
    Sell,
    SellShort,
    BuyToCover,
}
impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Buy => "Buy",
            Self::Sell => "Sell",
            Self::SellShort => "SellShort",
            Self::BuyToCover => "BuyToCover",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Market,
    Limit,
    Stop,
    Close,
}
#[derive(Clone, Debug)]
pub struct OrderSpec {
    pub total: bool,
    pub action: Action,
    pub label: String,
    pub quantity: Option<Expr>,
    pub price: Option<Expr>,
    pub kind: Kind,
}
#[derive(Clone, Debug)]
pub struct Order {
    pub total: bool,
    pub action: Action,
    pub label: String,
    pub quantity: Option<i64>,
    pub price: Option<f64>,
    pub kind: Kind,
}
#[derive(Clone, Debug)]
pub struct Program {
    pub inputs: BTreeMap<String, f64>,
    pub variables: BTreeMap<String, f64>,
    pub arrays: BTreeMap<String, Vec<f64>>,
    /// 由指定策略原生證據提供；Simple在歷史context中仍讀目前棒的值。
    pub simple_variables: HashSet<String>,
    /// 已核對 StorageType=16，且為無狀態宿主純算式的 Simple 函式。
    pub simple_host_functions: HashSet<String>,
    pub body: Vec<Stmt>,
    pub functions: BTreeMap<String, Program>,
    /// Typed function arguments, in declaration order (not strategy overrides).
    pub parameters: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct Token {
    s: String,
    line: usize,
    string: bool,
}
fn lex(source: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = source.trim_start_matches('\u{feff}').chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut out = vec![];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if c == '\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '{' || (c == '(' && chars.get(i + 1) == Some(&'*')) {
            let brace = c == '{';
            i += if brace { 1 } else { 2 };
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '\n' {
                    line += 1;
                }
                if (brace && chars[i] == '}')
                    || (!brace && chars[i] == '*' && chars.get(i + 1) == Some(&')'))
                {
                    i += if brace { 1 } else { 2 };
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(format!("第 {line} 行：註解未結束"));
            }
            continue;
        }
        if c == '"' {
            let start = line;
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        s.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                }
                s.push(chars[i]);
                i += 1;
            }
            if !closed {
                return Err(format!("第 {start} 行：字串未結束"));
            }
            out.push(Token {
                s,
                line: start,
                string: true,
            });
            continue;
        }
        let start = i;
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
        } else if "+-*/(),;:[]=<>".contains(c) {
            i += 1;
            if (c == '<' && chars.get(i) == Some(&'>'))
                || ("<>".contains(c) && chars.get(i) == Some(&'='))
            {
                i += 1;
            }
        } else {
            return Err(format!("第 {line} 行：不支援的字元 {c}"));
        }
        out.push(Token {
            s: chars[start..i]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
            line,
            string: false,
        });
    }
    out.push(Token {
        s: "<eof>".into(),
        line,
        string: false,
    });
    Ok(out)
}
struct Parser {
    tokens: Vec<Token>,
    i: usize,
    id: usize,
    arrays: HashSet<String>,
}
impl Parser {
    fn peek(&self) -> &str {
        &self.tokens[self.i].s
    }
    fn take(&mut self) -> Token {
        let t = self.tokens[self.i].clone();
        if self.i + 1 < self.tokens.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.peek() == s && !self.tokens[self.i].string {
            self.take();
            true
        } else {
            false
        }
    }
    fn error(&self, s: &str) -> String {
        format!("第 {} 行（{}）：{s}", self.tokens[self.i].line, self.peek())
    }
    fn need(&mut self, s: &str) -> Result<(), String> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.error(&format!("預期 {s}")))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        let t = self.take();
        if t.string
            || !t
                .s
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            Err(self.error("預期名稱"))
        } else {
            Ok(t.s)
        }
    }
    fn expr_node(&mut self, node: Node) -> Expr {
        let id = self.id;
        self.id += 1;
        Expr { id, node }
    }
    fn expression(&mut self, min: u8) -> Result<Expr, String> {
        let mut lhs = if self.eat("-") {
            let e = self.expression(7)?;
            self.expr_node(Node::Unary("-".into(), Box::new(e)))
        } else if self.eat("+") {
            self.expression(7)?
        } else if self.eat("not") {
            let e = self.expression(3)?;
            self.expr_node(Node::Unary("not".into(), Box::new(e)))
        } else if self.eat("(") {
            let e = self.expression(0)?;
            self.need(")")?;
            e
        } else {
            let t = self.take();
            if t.string {
                self.expr_node(Node::Text(t.s))
            } else if let Ok(n) = t.s.parse::<f64>() {
                if !n.is_finite() {
                    return Err(self.error("數值必須有限"));
                }
                self.expr_node(Node::Number(n))
            } else if t.s == "true" || t.s == "false" {
                self.expr_node(Node::Number(if t.s == "true" { 1.0 } else { 0.0 }))
            } else if t
                .s
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                if self.eat("(") {
                    let mut args = vec![];
                    if !self.eat(")") {
                        loop {
                            args.push(self.expression(0)?);
                            if self.eat(")") {
                                break;
                            }
                            self.need(",")?;
                        }
                    }
                    self.expr_node(Node::Call(t.s, args))
                } else {
                    self.expr_node(Node::Name(t.s))
                }
            } else {
                return Err(self.error("預期運算式"));
            }
        };
        loop {
            if self.peek() == "[" {
                self.take();
                let offset = self.expression(0)?;
                self.need("]")?;
                lhs = match &lhs.node {
                    Node::Name(name) if self.arrays.contains(name) =>
                        self.expr_node(Node::ArrayIndex(name.clone(), Box::new(offset))),
                    _ => self.expr_node(Node::History(Box::new(lhs), Box::new(offset))),
                };
                continue;
            }
            let p = match self.peek() {
                "or" => 1,
                "and" => 2,
                "=" | "<>" | ">" | "<" | ">=" | "<=" | "crosses" => 3,
                "+" | "-" => 4,
                "*" | "/" => 5,
                _ => 0,
            };
            if p == 0 || p < min {
                break;
            }
            let mut op = self.take().s;
            if op == "crosses" {
                if self.eat("above") { op = "crosses_above".into(); }
                else if self.eat("below") { op = "crosses_below".into(); }
                else { return Err(self.error("crosses 後須為 above 或 below")); }
            }
            let rhs = self.expression(p + 1)?;
            lhs = self.expr_node(Node::Binary(op, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }
    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        if self.eat("begin") {
            let mut out = vec![];
            while !self.eat("end") {
                if self.peek() == "<eof>" {
                    return Err(self.error("begin 未配對 end"));
                }
                out.extend(self.block()?);
            }
            self.eat(";");
            Ok(out)
        } else {
            Ok(vec![self.statement()?])
        }
    }
    fn statement(&mut self) -> Result<Stmt, String> {
        if self.eat("for") {
            let name = self.ident()?;
            self.need("=")?;
            let start = self.expression(0)?;
            let descending = if self.eat("downto") { true }
                else if self.eat("to") { false }
                else { return Err(self.error("for 需要 to 或 downto")); };
            let end = self.expression(0)?;
            self.need("begin")?;
            let mut body = vec![];
            while !self.eat("end") {
                if self.peek() == "<eof>" { return Err(self.error("for begin 未配對 end")); }
                body.extend(self.block()?);
            }
            self.eat(";");
            return Ok(Stmt::For(name, start, end, descending, body));
        }
        if self.eat("if") {
            let e = self.expression(0)?;
            self.need("then")?;
            let yes = self.block()?;
            let no = if self.eat("else") {
                self.block()?
            } else {
                vec![]
            };
            return Ok(Stmt::If(e, yes, no));
        }
        let name = self.ident()?;
        if name == "setfpcompareaccuracy" {
            self.need("(")?;
            let mode = if self.eat("fpcexact") || self.eat("5") {
                Stmt::SetExactComparison
            } else if self.eat("fpcmedaccuracy") || self.eat("2") {
                Stmt::SetMediumComparison
            } else {
                return Err(
                    self.error("目前支援 SetFPCompareAccuracy(FPCExact／5) 或 (FPCMedAccuracy／2)")
                );
            };
            self.need(")")?;
            self.eat(";");
            return Ok(mode);
        }
        let action = match name.as_str() {
            "buy" => Some(Action::Buy),
            "sell" => Some(Action::Sell),
            "sellshort" => Some(Action::SellShort),
            "buytocover" => Some(Action::BuyToCover),
            _ => None,
        };
        if let Some(action) = action {
            let label = if self.eat("(") {
                let t = self.take();
                if !t.string {
                    return Err(self.error("委託標籤須為字串"));
                }
                self.need(")")?;
                t.s
            } else {
                // 本機MC原生未命名SellShort委託（含反轉平倉）顯示Short。
                // Action::name仍保留送單動作名稱，顯式空字串不在此改寫。
                if action == Action::SellShort { "Short".into() } else { action.name().into() }
            };
            let quantity = if !["next", "this"].contains(&self.peek()) {
                let q = self.expression(0)?;
                let word = self.take().s;
                if !["contract", "contracts", "share", "shares"].contains(&word.as_str()) {
                    return Err(self.error("數量後需接 contracts"));
                }
                Some(q)
            } else {
                None
            };
            let total = self.eat("total");
            if total && (quantity.is_none() || matches!(action, Action::Buy | Action::SellShort)) {
                return Err(self.error("Total僅適用有明示數量的平倉委託"));
            }
            let (kind, price) = if self.eat("this") {
                self.need("bar")?;
                self.eat("on");
                self.need("close")?;
                (Kind::Close, None)
            } else {
                self.need("next")?;
                self.need("bar")?;
                self.eat("at");
                if self.eat("market") || self.eat("open") {
                    (Kind::Market, None)
                } else {
                    let p = self.expression(0)?;
                    let k = if self.eat("stop") {
                        Kind::Stop
                    } else if self.eat("limit") {
                        Kind::Limit
                    } else {
                        return Err(self.error("僅支援 market／stop／limit"));
                    };
                    (k, Some(p))
                }
            };
            self.eat(";");
            return Ok(Stmt::Order(OrderSpec {
                total,
                action,
                label,
                quantity,
                price,
                kind,
            }));
        }
        if self.eat("[") {
            if !self.arrays.contains(&name) { return Err(self.error("僅宣告陣列可使用索引寫入")); }
            let index = self.expression(0)?;
            self.need("]")?;
            self.need("=")?;
            let value = self.expression(0)?;
            self.eat(";");
            return Ok(Stmt::ArrayAssign(name, index, value));
        }
        self.need("=")?;
        let e = self.expression(0)?;
        self.eat(";");
        Ok(Stmt::Assign(name, e))
    }
}
fn static_number(e: &Expr) -> Result<f64, String> {
    match &e.node {
        Node::Number(x) => Ok(*x),
        Node::Unary(op, x) if op == "-" => Ok(-static_number(x)?),
        _ => Err("宣告初值須為數值常數".into()),
    }
}
const BUILTINS: &[&str] = &[
    "open",
    "high",
    "low",
    "close",
    "volume",
    "date",
    "time",
    "barnumber",
    "currentbar",
    "marketposition",
    "entryprice",
    "currentcontracts",
    "sessionlastbar",
    "ticks",
    "bigpointvalue",
    "pricescale",
    "minmove",
    "truerange",
    "netprofit",
    "totaltrades",
    "obv",
];
// NH14 Diag1原生逐棒觀察：394棒、18種組合中，僅Length截尾、
// 歷史索引就近取整與Series長度能完全重現entryp5。既有整數行為不變。
fn history_offset(value: f64) -> Result<usize, String> {
    if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
        return Err("歷史索引須為0至1000000有限數值，不可讀取未來".into());
    }
    Ok(value.round() as usize)
}
fn window_length(value: f64) -> Result<usize, String> {
    if !value.is_finite() || !(1.0..=100_000.0).contains(&value) {
        return Err("指標長度須為1至100000有限數值；小數部分截尾".into());
    }
    Ok(value.trunc() as usize)
}
const FUNCTIONS: &[(&str, usize)] = &[
    ("swinghigh", 4),
    ("swinglow", 4),
    ("highestbar", 2),
    ("lowestbar", 2),
    ("summation", 2),
    ("linearregslope", 2),
    ("countif", 2),
    ("intportion", 1),
    ("power", 2),
    ("exitprice", 1),
    ("barssinceentry", 1),
    ("avgtruerange", 1),
    ("round", 2),
    ("exitname", 1),
    ("entryname", 1),
    ("barssinceexit", 1),
    ("average", 2),
    ("xaverage", 2),
    ("rsi", 2),
    ("stddev", 2),
    ("averagefc", 2),
    ("highestfc", 2),
    ("lowestfc", 2),
    ("iff", 3),
    ("entryprice", 1),
    ("entriestoday", 1),
    ("openentrymaxprofit", 1),
    ("openentrycomission", 1),
    ("maxpositionprofit", 1),
    ("highest", 2),
    ("lowest", 2),
    ("absvalue", 1),
    ("minlist", 2),
    ("maxlist", 2),
    ("marketposition", 1),
    ("month", 1),
    ("dayofmonth", 1),
    ("dayofweek", 1),
];
fn validate_expr(
    e: &Expr,
    names: &HashSet<String>,
    functions: &BTreeMap<String, Program>,
    unknown: &mut HashSet<String>,
) -> Result<(), String> {
    match &e.node {
        Node::Name(n) => {
            if !names.contains(n) && !BUILTINS.contains(&n.as_str()) {
                unknown.insert(n.clone());
            }
        }
        Node::Call(n, args) => {
            if let Some((_, arity)) = FUNCTIONS.iter().find(|(s, _)| s == n) {
                if if n=="minlist" || n=="maxlist" { !(2..=100).contains(&args.len()) } else { args.len() != *arity } {
                    return Err(format!("{n} 需要 {arity} 個參數"));
                }
            } else if let Some(f) = functions.get(n) {
                if args.len() != f.parameters.len() {
                    return Err(format!("{n} 需要 {} 個參數", f.parameters.len()));
                }
            } else {
                unknown.insert(n.clone());
            }
            for a in args {
                validate_expr(a, names, functions, unknown)?;
            }
        }
        Node::History(a, b) | Node::Binary(_, a, b) => {
            validate_expr(a, names, functions, unknown)?;
            validate_expr(b, names, functions, unknown)?;
        }
        Node::Unary(_, a) => validate_expr(a, names, functions, unknown)?,
        Node::ArrayIndex(_, a) => validate_expr(a, names, functions, unknown)?,
        _ => {}
    }
    Ok(())
}
impl Program {
    pub fn set_function_storage_types(&mut self, types: &BTreeMap<String, u8>) -> Result<(), String> {
        // StorageType來自已綁定來源的PLA簽章，不由函式名稱或績效推定。
        // 目前只開放無參數、無區域狀態、單一宿主算式的Simple呼叫：
        // 宿主持倉在一次策略計算中不變，歷史context仍求當棒宿主算式。
        // 含行情/歷史/呼叫相依/持續狀態的Simple函式尚未驗證，明確拒絕。
        fn pure(e: &Expr, p: &Program) -> bool {
            match &e.node {
                Node::Number(_) => true,
                Node::Name(n) => p.inputs.contains_key(n) || ["true", "false", "currentcontracts", "currentshares"].contains(&n.as_str()),
                Node::Unary(_, a) => pure(a, p),
                Node::Binary(_, a, b) => pure(a, p) && pure(b, p),
                Node::Call(n, args) => ["marketposition", "entryprice", "absvalue", "intportion", "minlist", "maxlist", "power"].contains(&n.as_str())
                    && args.iter().all(|a| pure(a, p)),
                _ => false,
            }
        }
        let mut normalized = BTreeMap::new();
        for (name, storage) in types {
            let name = name.to_ascii_lowercase();
            let p = self.functions.get(&name).ok_or_else(|| format!("函式儲存型態指定到未知函式：{name}"))?;
            if normalized.insert(name.clone(), *storage).is_some() { return Err(format!("函式儲存型態不分大小寫重複：{name}")); }
            if ![16, 32].contains(storage) { return Err(format!("不支援的函式StorageType：{name}={storage}")); }
            if *storage == 16 && !(p.parameters.is_empty() && p.variables.len() == 1 && p.arrays.is_empty()
                && matches!(p.body.as_slice(), [Stmt::Assign(n, e)] if n == &name && pure(e, p))) {
                return Err(format!("Simple函式尚不支援此來源；僅接受無參數、無區域狀態的宿主純算式：{name}"));
            }
        }
        fn apply(p: &mut Program, types: &BTreeMap<String, u8>) {
            p.simple_host_functions = p.functions.keys().filter(|n| types.get(*n) == Some(&16)).cloned().collect();
            for child in p.functions.values_mut() { apply(child, types); }
        }
        apply(self, &normalized);
        Ok(())
    }
    pub fn set_simple_variables(&mut self, names: &[String]) -> Result<(), String> {
        let mut simple=HashSet::new();
        for name in names {
            let name=name.to_ascii_lowercase();
            if !self.variables.contains_key(&name) { return Err(format!("Simple型別指定到未宣告變數：{name}")); }
            if !simple.insert(name.clone()) {return Err(format!("Simple變數設定重複：{name}"));}
        }
        self.simple_variables=simple;
        Ok(())
    }
    pub fn compile(source: &str, overrides: &BTreeMap<String, f64>) -> Result<Self, String> {
        Self::compile_inner(source, overrides, &BTreeMap::new(), None)
    }
    pub fn with_functions(
        source: &str,
        overrides: &BTreeMap<String, f64>,
        functions: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let mut sources = BTreeMap::new();
        for (name, body) in functions {
            let name = name.to_ascii_lowercase();
            if name.is_empty()
                || !name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                || BUILTINS.contains(&name.as_str())
                || FUNCTIONS.iter().any(|(n, _)| *n == name)
            {
                return Err(format!("自訂函式名稱無效或使用保留名稱：{name}"));
            }
            if sources.insert(name.clone(),body.clone()).is_some() {
                return Err(format!("自訂函式名稱不分大小寫重複：{name}"));
            }
        }
        // Compile each supplied dependency before its caller. Previously every
        // function was compiled with an empty registry, rejecting nested calls.
        // Inspect lexer tokens so comments/string literals cannot create edges.
        fn compile_function(name:&str,sources:&BTreeMap<String,String>,compiled:&mut BTreeMap<String,Program>,stack:&mut Vec<String>)->Result<(),String> {
            if compiled.contains_key(name) {return Ok(());}
            if stack.iter().any(|item|item==name) {
                return Err(format!("自訂函式循環相依：{} -> {name}",stack.join(" -> ")));
            }
            if stack.len()>=32 {return Err("自訂函式相依深度超過32層".into());}
            stack.push(name.into());
            let tokens=lex(&sources[name])?;
            let mut dependencies=BTreeMap::new();
            for (at,token) in tokens.iter().enumerate() {
                if token.string {continue;}
                if token.s==name {
                    if tokens.get(at+1).is_some_and(|next|next.s=="(") {
                        return Err(format!("自訂函式不可遞迴呼叫自己：{name}"));
                    }
                } else if sources.contains_key(&token.s) {
                    compile_function(&token.s,sources,compiled,stack)?;
                    dependencies.insert(token.s.clone(),compiled[&token.s].clone());
                }
            }
            let program=Program::compile_inner(&sources[name],&BTreeMap::new(),&dependencies,Some(name))?;
            stack.pop();compiled.insert(name.into(),program);Ok(())
        }
        let mut compiled=BTreeMap::new();
        for name in sources.keys() {compile_function(name,&sources,&mut compiled,&mut vec![])?;}
        Self::compile_inner(source, overrides, &compiled, None)
    }
    fn compile_inner(
        source: &str,
        overrides: &BTreeMap<String, f64>,
        functions: &BTreeMap<String, Program>,
        return_name: Option<&str>,
    ) -> Result<Self, String> {
        if source.len() > 100_000 {
            return Err("策略原始碼超過第一版支援上限 100000 bytes".into());
        }
        let mut p = Parser {
            tokens: lex(source)?,
            i: 0,
            id: 0,
            arrays: HashSet::new(),
        };
        let mut inputs = BTreeMap::new();
        let mut variables = BTreeMap::new();
        let mut arrays = BTreeMap::new();
        let mut parameters = vec![];
        while ["inputs", "input", "variables", "vars", "var", "variable", "arrays", "array"].contains(&p.peek()) {
            let declaration_kind = p.take().s;
            let is_input = ["inputs", "input"].contains(&declaration_kind.as_str());
            let is_array = ["arrays", "array"].contains(&declaration_kind.as_str());
            p.need(":")?;
            loop {
                let name = p.ident()?;
                if BUILTINS.contains(&name.as_str())
                    || functions.contains_key(&name)
                    || inputs.contains_key(&name)
                    || variables.contains_key(&name)
                    || arrays.contains_key(&name)
                    || parameters.iter().any(|(n, _)| n == &name)
                {
                    return Err(format!("名稱重複或保留字：{name}"));
                }
                let maximum = if is_array {
                    p.need("[")?;
                    let dimension = static_number(&p.expression(0)?)?;
                    p.need("]")?;
                    if dimension < 0.0 || dimension.fract()!=0.0 || dimension>100_000.0 {
                        return Err("陣列上界須為0至100000整數；維度包含索引0".into());
                    }
                    Some(dimension as usize)
                } else { None };
                p.need("(")?;
                if is_input && return_name.is_some() && ["numericseries", "numericsimple"].contains(&p.peek()) {
                    let kind = p.take().s;
                    p.need(")")?;
                    parameters.push((name, kind));
                    if p.eat(";") { break; }
                    p.need(",")?;
                    continue;
                }
                let e = p.expression(0)?;
                p.need(")")?;
                let value = static_number(&e)?;
                if let Some(maximum) = maximum {
                    p.arrays.insert(name.clone());
                    arrays.insert(name, vec![value; maximum+1]);
                } else if is_input {
                    inputs.insert(name, value);
                } else {
                    variables.insert(name, value);
                }
                if p.eat(";") {
                    break;
                }
                p.need(",")?;
            }
        }
        for token in &p.tokens {
            if token
                .s
                .strip_prefix("condition")
                .and_then(|n| n.parse::<usize>().ok())
                .is_some_and(|n| (1..=99).contains(&n))
            {
                variables.entry(token.s.clone()).or_insert(0.0);
            }
        }
        if let Some(n) = return_name {
            variables.insert(n.into(), 0.0);
        }
        for (n, v) in overrides {
            let key = n.to_ascii_lowercase();
            if !v.is_finite() {
                return Err(format!("參數 {n} 須為有限數值"));
            }
            *inputs
                .get_mut(&key)
                .ok_or_else(|| format!("未宣告參數：{n}"))? = *v;
        }
        // MC 預先宣告的 Value1..99／Condition1..99；只登錄實際出現的
        // 保留名稱。一般未知變數仍拒絕，不能以這項規則代填任意缺值。
        for token in &p.tokens {
            if token.string {continue;}
            let reserved=["value","condition"].iter().any(|prefix|token.s.strip_prefix(prefix)
                .and_then(|suffix|suffix.parse::<u32>().ok().map(|n|(suffix,n)))
                .is_some_and(|(suffix,n)|(1..=99).contains(&n)&&suffix==n.to_string()));
            if reserved && !inputs.contains_key(&token.s) && !arrays.contains_key(&token.s)
                && !parameters.iter().any(|(n,_)|n==&token.s) {
                variables.entry(token.s.clone()).or_insert(0.0);
            }
        }
        let mut body = vec![];
        while p.peek() != "<eof>" {
            if p.eat(";") {
                continue;
            }
            body.extend(p.block()?);
        }
        let names: HashSet<String> = inputs
            .keys()
            .chain(variables.keys())
            .chain(arrays.keys())
            .chain(functions.keys())
            .chain(parameters.iter().map(|(n, _)| n))
            .cloned()
            .collect();
        let mut unknown = HashSet::new();
        fn visit(
            stmts: &[Stmt],
            names: &HashSet<String>,
            vars: &BTreeMap<String, f64>,
            functions: &BTreeMap<String, Program>,
            unknown: &mut HashSet<String>,
        ) -> Result<(), String> {
            for s in stmts {
                match s {
                    Stmt::SetExactComparison | Stmt::SetMediumComparison => {}
                    Stmt::Assign(n, e) => {
                        if !vars.contains_key(n) {
                            return Err(format!("變數未宣告或不可寫入：{n}"));
                        }
                        validate_expr(e, names, functions, unknown)?;
                    }
                    Stmt::ArrayAssign(_, index, value) => {
                        validate_expr(index, names, functions, unknown)?;
                        validate_expr(value, names, functions, unknown)?;
                    }
                    Stmt::For(n, start, end, _, body) => {
                        if !vars.contains_key(n) { return Err(format!("迴圈變數未宣告或不可寫入：{n}")); }
                        validate_expr(start, names, functions, unknown)?;
                        validate_expr(end, names, functions, unknown)?;
                        visit(body, names, vars, functions, unknown)?;
                    }
                    Stmt::If(e, a, b) => {
                        validate_expr(e, names, functions, unknown)?;
                        visit(a, names, vars, functions, unknown)?;
                        visit(b, names, vars, functions, unknown)?;
                    }
                    Stmt::Order(o) => {
                        for e in [&o.price, &o.quantity].into_iter().flatten() {
                            validate_expr(e, names, functions, unknown)?;
                        }
                    }
                }
            }
            Ok(())
        }
        visit(&body, &names, &variables, functions, &mut unknown)?;
        if !unknown.is_empty() {
            let mut list: Vec<_> = unknown.into_iter().collect();
            list.sort();
            return Err(format!(
                "缺少自訂函式定義或不支援的名稱：{}；不會代填回傳值或繼續回測",
                list.join(", ")
            ));
        }
        Ok(Self {
            inputs,
            variables,
            arrays,
            simple_variables: HashSet::new(),
            simple_host_functions: HashSet::new(),
            body,
            functions: functions.clone(),
            parameters,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExecutionContext {
    pub total_trades: Option<f64>,
    pub ticks: Option<f64>,
    pub big_point_value: Option<f64>,
    pub price_scale: Option<f64>,
    pub min_move: Option<f64>,
    pub session_last_bar: Option<bool>,
    pub recent_entry_dates: Vec<i32>,
    pub open_entry_max_profit: Option<f64>,
    pub open_entry_commission: Option<f64>,
    pub net_profit: Option<f64>,
    pub recent_exits: Vec<(usize, String)>,
    pub recent_exit_prices: Vec<f64>,
    pub entry_bar: Option<usize>,
    pub entry_name: Option<String>,
    pub recent_entry_names: Vec<String>,
}
pub struct Runtime<'a> {
    pub program: &'a Program,
    pub bars: &'a [Candle],
    pub index: usize,
    pub position: i64,
    pub entry_price: f64,
    pub contracts: i64,
    pub bar_offset: usize,
    pub execution: ExecutionContext,
    context_history: Vec<ExecutionContext>,
    fc_sums: RefCell<HashMap<(usize, usize, usize), f64>>,
    exponential_values: RefCell<HashMap<(usize, usize, usize), f64>>,
    rsi_values: RefCell<HashMap<(usize, usize), (f64, f64, usize)>>,
    comparison_epsilon: f64,
    vars: HashMap<String, Vec<Option<f64>>>,
    arrays: HashMap<String, Vec<Vec<Option<f64>>>>,
    functions: RefCell<BTreeMap<String, Runtime<'a>>>,
    call_functions: RefCell<HashMap<usize, Runtime<'a>>>,
    parameter_values: HashMap<String, Vec<Option<f64>>>,
}
impl<'a> Runtime<'a> {
    fn linear_reg_slope(&self, call: usize, args: &[Expr], i: usize) -> Result<Option<f64>, String> {
        // BuiltInFunctions.dll contains the original LinearRegSlope/LinearReg PL:
        // PriceValue is NumericSeries, Len is NumericSimple; var0 starts at zero.
        // The wrapper retains var0 when LinearReg returns -1 for Len <= 1.
        // Dedicated expression/bar keys preserve independent instances and history.
        const CACHE_KIND: usize = usize::MAX - 1;
        if i < self.bar_offset { return Ok(Some(0.0)); }
        if let Some(value) = self.exponential_values.borrow().get(&(call, CACHE_KIND, i)).copied() {
            return Ok(Some(value));
        }
        let previous = (self.bar_offset..i).rev().find_map(|at| {
            self.exponential_values.borrow().get(&(call, CACHE_KIND, at)).copied().map(|value| (at, value))
        });
        let (start, mut value) = previous.map_or((self.bar_offset, 0.0), |(at, value)| (at + 1, value));
        for at in start..=i {
            // Freeze this argument once per calculation bar, never once per lag.
            let Some(length) = self.evaluate(&args[1], at)? else { return Ok(None); };
            if !length.is_finite() || length > 100_000.0 {
                return Err("LinearRegSlope長度須為不大於100000的有限數值".into());
            }
            if length > 1.0 {
                let len = window_length(length)?;
                if at + 1 < len { return Ok(None); }
                let mut weighted = 0.0;
                let mut sum = 0.0;
                // Both the original for loop and Summation visit newest first.
                for ago in 0..len {
                    let Some(price) = self.evaluate(&args[0], at - ago)? else { return Ok(None); };
                    weighted += ago as f64 * price;
                    sum += price;
                }
                // Keep raw (possibly fractional) Len in the original algebra;
                // only the historical iteration count truncates.
                let sum_x = length * (length - 1.0) * 0.5;
                let sum_x2 = length * (length - 1.0) * (2.0 * length - 1.0) * (1.0 / 6.0);
                let denominator = sum_x * sum_x - length * sum_x2;
                if denominator == 0.0 { return Err("LinearRegSlope分母為零".into()); }
                value = (length * weighted - sum_x * sum) / denominator;
                if !value.is_finite() { return Err("LinearRegSlope產生非有限數值".into()); }
            }
            self.exponential_values.borrow_mut().insert((call, CACHE_KIND, at), value);
        }
        Ok(Some(value))
    }
    fn obv(&self, call: usize, i: usize) -> Result<Option<f64>, String> {
        if i < self.bar_offset { return Ok(None); }
        if let Some(value)=self.exponential_values.borrow().get(&(call,usize::MAX,i)).copied() { return Ok(Some(value)); }
        let previous=(self.bar_offset..i).rev().find_map(|at| self.exponential_values.borrow().get(&(call,usize::MAX,at)).copied().map(|v|(at,v)));
        let (start,mut value)=previous.map_or((self.bar_offset,0.0),|(at,v)|(at+1,v));
        for at in start..=i {
            if at>0 {
                let volume=self.context_history.get(at).and_then(|c|c.ticks).ok_or("OBV缺少Build Volume On成交量上下文")?;
                if self.bars[at].close>self.bars[at-1].close { value+=volume; }
                else if self.bars[at].close<self.bars[at-1].close { value-=volume; }
            }
            self.exponential_values.borrow_mut().insert((call,usize::MAX,at),value);
        }
        Ok(Some(value))
    }
    fn array_index(&self, name: &str, index: &Expr, i: usize) -> Result<usize, String> {
        let length = self.program.arrays.get(name).ok_or_else(|| format!("陣列未宣告：{name}"))?.len();
        match self.evaluate(index, i)? {
            Some(n) if n >= 0.0 && n.fract()==0.0 && n < length as f64 => Ok(n as usize),
            _ => Err(format!("陣列{name}索引須為0至{}整數",length-1)),
        }
    }
    pub fn current_variable(&self, name: &str) -> Option<f64> {
        self.vars
            .get(name)
            .and_then(|v| v.get(self.index))
            .copied()
            .flatten()
    }
    pub fn new(program: &'a Program, bars: &'a [Candle]) -> Self {
        Self {
            program,
            bars,
            index: 0,
            position: 0,
            entry_price: 0.0,
            contracts: 0,
            bar_offset: 0,
            execution: ExecutionContext::default(),
            context_history: vec![],
            fc_sums: RefCell::new(HashMap::new()),
            exponential_values: RefCell::new(HashMap::new()),
            rsi_values: RefCell::new(HashMap::new()),
            // CBaseUserStudyData::__fp_accuracy_value(2)；策略可再明示改成 Exact。
            comparison_epsilon: f64::from_bits(0x3d83880000000000),
            vars: program
                .variables
                .iter()
                .map(|(n, _)| (n.clone(), Vec::new()))
                .collect(),
            arrays: program.arrays.keys().map(|n| (n.clone(), vec![])).collect(),
            functions: RefCell::new(
                program
                    .functions
                    .iter()
                    .filter(|(_, p)| p.parameters.is_empty())
                    .map(|(n, p)| (n.clone(), Runtime::new(p, bars)))
                    .collect(),
            ),
            call_functions: RefCell::new(HashMap::new()),
            parameter_values: HashMap::new(),
        }
    }
    fn function_value(&self, name: &str, i: usize) -> Result<Option<f64>, String> {
        let f = self.functions.borrow();
        let rt = f.get(name).ok_or_else(|| format!("缺少函式 {name}"))?;
        if self.program.simple_host_functions.contains(name) {
            // SDK SimpleFunction::operator()每次執行Body，不取Series的歷史回傳值。
            // 僅支援已檢驗的單一宿主算式，故可直接在當棒宿主context求值。
            // 沒有被呼叫時不執行，保留條件分支內的除零等行為。
            let Stmt::Assign(_, expression) = &rt.program.body[0] else { unreachable!() };
            return rt.evaluate(expression, self.index);
        }
        Ok(rt.vars.get(name).and_then(|v| v.get(i)).copied().flatten())
    }
    fn argument_function_value(&self, call: usize, name: &str, args: &[Expr], i: usize) -> Result<Option<f64>, String> {
        let definition = &self.program.functions[name];
        let start = self.call_functions.borrow().get(&call)
            .and_then(|rt| rt.parameter_values.values().next().map(Vec::len)).unwrap_or(0);
        let mut additions = vec![];
        for arg in args {
            let mut values = vec![];
            for at in start..=i { values.push(self.evaluate(arg, at)?); }
            additions.push(values);
        }
        let mut instances = self.call_functions.borrow_mut();
        let rt = instances.entry(call).or_insert_with(|| Runtime::new(definition, self.bars));
        for ((name, _), values) in definition.parameters.iter().zip(additions) {
            rt.parameter_values.entry(name.clone()).or_default().extend(values);
        }
        rt.bar_offset = self.bar_offset;
        // Series arguments and local histories belong to this call site. Fill
        // skipped calculation bars chronologically; never share DF instances.
        for at in rt.context_history.len().max(self.bar_offset)..=i {
            rt.execution = self.context_history.get(at).cloned().ok_or("函式缺少歷史成交上下文")?;
            let get = |n: &str| self.vars.get(n).and_then(|v| v.get(at)).copied().flatten().unwrap_or(0.0);
            rt.step(at, get("@marketposition") as i64, get("@entryprice"), get("@currentcontracts") as i64)?;
        }
        Ok(rt.vars.get(name).and_then(|v| v.get(i)).copied().flatten())
    }
    fn exit_ordinal(&self, arg: &Expr, i: usize) -> Result<usize, String> {
        match self.evaluate(arg, i)? {
            Some(n) if n.fract() == 0.0 && (1.0..=10.0).contains(&n) => Ok(n as usize),
            _ => Err("平倉歷史索引須為 1 至 10".into()),
        }
    }
    fn text_value(&self, e: &Expr, i: usize) -> Result<Option<String>, String> {
        match &e.node {
            Node::Text(s) => Ok(Some(s.clone())),
            Node::Call(n, args) if n == "exitname" => {
                let n = self.exit_ordinal(&args[0], i)?;
                let exits = &self.context_history.get(i).ok_or("ExitName 缺少成交上下文")?.recent_exits;
                Ok(Some(exits.iter().rev().nth(n - 1).map(|(_, s)| s.clone()).unwrap_or_default()))
            }
            Node::Call(n,args) if n=="entryname" => {
                let ordinal=match self.evaluate(&args[0],i)? {
                    Some(n) if n.fract()==0.0 && (0.0..=10.0).contains(&n)=>n as usize,
                    _=>return Err("EntryName 目前支援 0 至 10 的整數部位索引".into()),
                };
                let context=self.context_history.get(i).ok_or("EntryName 缺少成交上下文")?;
                Ok(Some(if ordinal==0 {context.entry_name.clone().unwrap_or_default()}
                    else {context.recent_entry_names.iter().rev().nth(ordinal-1).cloned().unwrap_or_default()}))
            }
            Node::History(a, b) => match self.evaluate(b, i)? {
                Some(n) => {let offset=history_offset(n)?;if i>=offset {self.text_value(a,i-offset)}else{Ok(None)}},
                None => Ok(None),
            },
            _ => Ok(None),
        }
    }
    pub fn evaluate(&self, e: &Expr, i: usize) -> Result<Option<f64>, String> {
        let value = match &e.node {
            Node::Number(n) => Some(*n),
            Node::Text(_) => return Err("字串僅支援 EntryName／ExitName 的等值／不等值比較".into()),
            Node::ArrayIndex(name, index) => {
                let index = self.array_index(name, index, i)?;
                self.arrays.get(name).and_then(|h| h.get(i)).and_then(|a| a.get(index)).copied().flatten()
            }
            Node::Name(n) => {
                if let Some(history) = self.parameter_values.get(n) {
                    let at=if self.program.parameters.iter().any(|(name,kind)|name==n && kind=="numericsimple") {self.index}else{i};
                    history.get(at).copied().flatten()
                } else if let Some(v) = self.program.inputs.get(n) {
                    Some(*v)
                } else if let Some(history) = self.vars.get(n) {
                    history.get(if self.program.simple_variables.contains(n) {self.index}else{i}).copied().flatten()
                } else {
                    let b = &self.bars[i];
                    match n.as_str() {
                        "obv" => self.obv(e.id,i)?,
                        "open" => Some(b.open),
                        "high" => Some(b.high),
                        "low" => Some(b.low),
                        "close" => Some(b.close),
                        "volume" => Some(b.volume),
                        "truerange" => i.checked_sub(1).map(|previous| {
                            b.high.max(self.bars[previous].close)
                                - b.low.min(self.bars[previous].close)
                        }),
                        "date" => Some(b.pl_date as f64),
                        "time" => Some(b.pl_time as f64),
                        "barnumber" | "currentbar" => {
                            Some(i.saturating_sub(self.bar_offset) as f64 + 1.0)
                        }
                        "marketposition" => self.vars["@marketposition"].get(i).copied().flatten(),
                        "entryprice" => self.vars["@entryprice"].get(i).copied().flatten(),
                        "currentcontracts" => {
                            self.vars["@currentcontracts"].get(i).copied().flatten()
                        }
                        "netprofit" => Some(self.context_history.get(i).and_then(|c| c.net_profit).ok_or("NetProfit 缺少已平倉損益上下文")?),
                        "totaltrades" => Some(self.context_history.get(i).and_then(|c| c.total_trades).ok_or("TotalTrades缺少已平倉筆數上下文")?),
                        "sessionlastbar" => Some(
                            self.context_history
                                .get(i)
                                .and_then(|x| x.session_last_bar)
                                .ok_or("SessionLastBar 需要明確交易時段及結束時間標記")?
                                as u8 as f64,
                        ),
                        "ticks" => Some(
                            self.context_history
                                .get(i)
                                .and_then(|x| x.ticks)
                                // 暖機行情可供指標回看，但不執行策略。只有宿主已明示
                                // trade_volume的Ticks時，才使用同一棒原始成交量。
                                .or_else(|| if i<self.bar_offset && self.execution.ticks.is_some() {Some(b.volume)}else{None})
                                .ok_or("Ticks 缺少明示的 Build Volume On 設定或來源資料")?,
                        ),
                        "bigpointvalue" => Some(
                            self.context_history
                                .get(i)
                                .and_then(|x| x.big_point_value)
                                .ok_or("BigPointValue 缺少契約規格")?,
                        ),
                        "pricescale" => Some(self.context_history.get(i).and_then(|x|x.price_scale).ok_or("PriceScale 缺少商品價格刻度")?),
                        "minmove" => Some(self.context_history.get(i).and_then(|x|x.min_move).ok_or("MinMove 缺少商品最小移動單位")?),
                        _ => self.function_value(n, i)?,
                    }
                }
            }
            Node::Unary(op, a) => self.evaluate(a, i)?.map(|v| {
                if op == "-" {
                    -v
                } else {
                    if v == 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            }),
            Node::History(a, b) => match self.evaluate(b, i)? {
                Some(value) => {
                    let offset=history_offset(value)?;
                    if i >= offset {
                        self.evaluate(a, i - offset)?
                    } else {
                        None
                    }
                }
                None => None,
            },
            Node::Binary(op, a, b) if op == "crosses_above" || op == "crosses_below" => {
                if i == 0 { Some(0.0) } else {
                    match (self.evaluate(a, i)?, self.evaluate(b, i)?, self.evaluate(a, i-1)?, self.evaluate(b, i-1)?) {
                        (Some(x), Some(y), Some(px), Some(py)) => Some(if op == "crosses_above" { (x > y && px <= py) as u8 as f64 } else { (x < y && px >= py) as u8 as f64 }),
                        _ => Some(0.0),
                    }
                }
            }
            Node::Binary(op, a, b) if ["=", "<>"].contains(&op.as_str()) && (self.text_value(a, i)?.is_some() || self.text_value(b, i)?.is_some()) => {
                match (self.text_value(a, i)?, self.text_value(b, i)?) {
                    (Some(x), Some(y)) => Some(((x == y) == (op == "=")) as u8 as f64),
                    _ => return Err("字串與數值不可比較".into()),
                }
            }
            Node::Binary(op, a, b) => match (self.evaluate(a, i)?, self.evaluate(b, i)?) {
                (Some(x), Some(y)) => Some(match op.as_str() {
                    "+" => x + y,
                    "-" => x - y,
                    "*" => x * y,
                    "/" => {
                        if y == 0.0 {
                            return Err("策略除以零".into());
                        }
                        x / y
                    }
                    "=" => ((x - y).abs() <= self.comparison_epsilon) as u8 as f64,
                    "<>" => ((x - y).abs() > self.comparison_epsilon) as u8 as f64,
                    ">" => (x > y && (x - y).abs() > self.comparison_epsilon) as u8 as f64,
                    "<" => (x < y && (x - y).abs() > self.comparison_epsilon) as u8 as f64,
                    ">=" => (x > y || (x - y).abs() <= self.comparison_epsilon) as u8 as f64,
                    "<=" => (x < y || (x - y).abs() <= self.comparison_epsilon) as u8 as f64,
                    "and" => (x != 0.0 && y != 0.0) as u8 as f64,
                    "or" => (x != 0.0 || y != 0.0) as u8 as f64,
                    _ => unreachable!(),
                }),
                _ => None,
            },
            Node::Call(n, args) => {
                if self.program.functions.contains_key(n) {
                    if args.is_empty() { self.function_value(n, i)? }
                    else { self.argument_function_value(e.id, n, args, i)? }
                } else if n == "linearregslope" {
                    self.linear_reg_slope(e.id, args, i)?
                } else if n == "barssinceexit" {
                    let n = self.exit_ordinal(&args[0], i)?;
                    let exits = &self.context_history.get(i).ok_or("BarsSinceExit 缺少成交上下文")?.recent_exits;
                    Some(exits.iter().rev().nth(n - 1).map_or(-1.0, |(at, _)| i.saturating_sub(*at) as f64))
                } else if n == "exitprice" {
                    let ordinal=self.exit_ordinal(&args[0],i)?;
                    let context=self.context_history.get(i).ok_or("ExitPrice缺少成交上下文")?;
                    Some(context.recent_exit_prices.iter().rev().nth(ordinal-1).copied().unwrap_or(0.0))
                } else if n == "barssinceentry" {
                    if self.evaluate(&args[0],i)?!=Some(0.0) { return Err("BarsSinceEntry僅支援目前單一部位(0)".into()); }
                    Some(self.context_history.get(i).ok_or("BarsSinceEntry缺少成交上下文")?.entry_bar.map_or(-1.0,|at|i.saturating_sub(at) as f64))
                } else if n == "intportion" {
                    self.evaluate(&args[0],i)?.map(f64::trunc)
                } else if n == "power" {
                    match (self.evaluate(&args[0],i)?,self.evaluate(&args[1],i)?) {
                        (Some(a),Some(b))=>Some(a.powf(b)), _=>None,
                    }
                } else if n == "minlist" || n == "maxlist" {
                    let mut values=vec![];
                    for arg in args { let Some(value)=self.evaluate(arg,i)? else {return Ok(None)}; values.push(value); }
                    Some(values.into_iter().reduce(|a,b|if n=="minlist" {a.min(b)}else{a.max(b)}).unwrap())
                } else if n == "swinghigh" || n == "swinglow" {
                    let integer = |value:Option<f64>,field:&str,minimum:f64| match value {
                        Some(n) if n.fract()==0.0 && (minimum..=100_000.0).contains(&n)=>Ok(n as usize),
                        _=>Err(format!("Swing的{field}須為{minimum}至100000整數")),
                    };
                    let instance=integer(self.evaluate(&args[0],i)?,"Instance",1.0)?;
                    let strength=integer(self.evaluate(&args[2],i)?,"Strength",0.0)?;
                    let length=integer(self.evaluate(&args[3],i)?,"Length",0.0)?;
                    // 原廠 Pivot 在 RightStrength>=Len 時不進迴圈，回傳-1。
                    if length<=strength {return Ok(Some(-1.0));}
                    let mut found=0;
                    let mut answer=-1.0;
                    for ago in strength..length {
                        if i<ago+strength {continue;}
                        let pivot=i-ago;
                        let Some(price)=self.evaluate(&args[1],pivot)? else {continue;};
                        let mut qualifies=true;
                        for distance in 1..=strength {
                            let (Some(left),Some(right))=(self.evaluate(&args[1],pivot-distance)?,self.evaluate(&args[1],pivot+distance)?) else {qualifies=false;break;};
                            // EasyLanguage官方定義：較舊側可相等，較新側必須嚴格突破。
                            if if n=="swinghigh" {price<left || price<=right} else {price>left || price>=right} {qualifies=false;break;}
                        }
                        if qualifies {found+=1;if found==instance {answer=price;break;}}
                    }
                    Some(answer)
                } else if n == "highestbar" || n == "lowestbar" || n == "countif" {
                    let Some(length)=self.evaluate(&args[1],i)? else {return Ok(None)};
                    let length=window_length(length)?;
                    if i+1<length {None} else {
                        let mut best=None;
                        let mut selected=0usize;
                        let mut count=0usize;
                        for ago in 0..length {
                            let Some(value)=self.evaluate(&args[0],i-ago)? else {return Ok(None);};
                            if value!=0.0 {count+=1;}
                            if best.is_none() || if n=="highestbar" {value>best.unwrap()} else {value<best.unwrap()} {best=Some(value);selected=ago;}
                        }
                        Some(if n=="countif" {count as f64}else{selected as f64})
                    }
                } else if n == "rsi" {
                    // Wilder seed and recurrence from the official EasyLanguage RSI
                    // specification. Native MultiCharts point-by-point parity remains
                    // unverified; do not infer its initialization from trade results.
                    if i < self.bar_offset { return Ok(None); }
                    let Some(length)=self.evaluate(&args[1],i)? else {return Ok(None)};
                    if !length.is_finite() || length.fract()!=0.0 || !(1.0..=100_000.0).contains(&length) {
                        return Err("RSI 長度目前須為 1 至 100000 的固定整數".into());
                    }
                    let len=length as usize;
                    let start=self.bar_offset.max(len);
                    if i<start {return Ok(None);}
                    let previous=(start..=i).rev().find_map(|at|self.rsi_values.borrow().get(&(e.id,at)).copied().map(|state|(at,state)));
                    let (mut at,mut up,mut down)=if let Some((at,(up,down,original_len)))=previous {
                        if original_len!=len {return Err("RSI 尚未驗證跨長度變動，請使用固定長度".into());}
                        (at,up,down)
                    } else {
                        let mut up=0.0;let mut down=0.0;
                        for ago in 0..len {
                            let index=start-ago;
                            let (Some(current),Some(previous))=(self.evaluate(&args[0],index)?,self.evaluate(&args[0],index-1)?) else {return Ok(None);};
                            let change=current-previous;up+=change.max(0.0);down+=(-change).max(0.0);
                        }
                        up/=length;down/=length;
                        self.rsi_values.borrow_mut().insert((e.id,start),(up,down,len));
                        (start,up,down)
                    };
                    while at<i {
                        at+=1;
                        if self.evaluate(&args[1],at)?!=Some(length){return Err("RSI 尚未驗證跨長度變動，請使用固定長度".into());}
                        let (Some(current),Some(previous))=(self.evaluate(&args[0],at)?,self.evaluate(&args[0],at-1)?) else {return Ok(None);};
                        let change=current-previous;
                        up=(up*(length-1.0)+change.max(0.0))/length;
                        down=(down*(length-1.0)+(-change).max(0.0))/length;
                        self.rsi_values.borrow_mut().insert((e.id,at),(up,down,len));
                    }
                    if up+down==0.0 {return Err("RSI 初始區間完全無價格變動；原生 MC 的零分母慣例尚未驗證，不代填數值".into());}
                    Some(100.0*up/(up+down))
                } else if n == "avgtruerange" {
                    let Some(length)=self.evaluate(&args[0],i)? else {return Ok(None)};
                    let len=window_length(length)?;
                    if i < len { None } else {
                        let mut total = 0.0;
                        for ago in 0..len {let at=i-ago; total += self.bars[at].high.max(self.bars[at-1].close) - self.bars[at].low.min(self.bars[at-1].close); }
                        // 原廠 AvgTrueRange -> Average -> Summation：迴圈截尾，除數保留原始 Len。
                        Some(total / length)
                    }
                } else if n == "round" {
                    match (self.evaluate(&args[0], i)?, self.evaluate(&args[1], i)?) {
                        (Some(x), Some(d)) if d.fract() == 0.0 && (-15.0..=15.0).contains(&d) => { let f=10f64.powi(d as i32); Some((x*f).round()/f) },
                        (None, _) | (_, None) => None,
                        _ => return Err("Round 小數位數須為 -15 至 15 的整數".into()),
                    }
                } else if ["month", "dayofmonth", "dayofweek"].contains(&n.as_str()) {
                    match self.evaluate(&args[0], i)? {
                        Some(v) if v.fract() == 0.0 => {
                            let v = v as i32;
                            let date = NaiveDate::from_ymd_opt(
                                v / 10000 + 1900,
                                (v / 100 % 100) as u32,
                                (v % 100) as u32,
                            )
                            .ok_or("無效 PowerLanguage 日期")?;
                            Some(match n.as_str() {
                                "month" => date.month() as f64,
                                "dayofmonth" => date.day() as f64,
                                _ => date.weekday().num_days_from_sunday() as f64,
                            })
                        }
                        _ => None,
                    }
                } else if n == "iff" {
                    let test = self.evaluate(&args[0], i)?;
                    match test { Some(x) => self.evaluate(&args[if x != 0.0 {1} else {2}], i)?, None => None }
                } else if n == "entriestoday" {
                    match self.evaluate(&args[0], i)? {
                        Some(date) if date.fract() == 0.0 => Some(
                            self.context_history
                                .get(i)
                                .ok_or("EntriesToday 缺少成交上下文")?
                                .recent_entry_dates
                                .iter()
                                .rev()
                                .take(10)
                                .filter(|d| **d == date as i32)
                                .count() as f64,
                        ),
                        _ => return Err("EntriesToday 需要整數 PowerLanguage 日期".into()),
                    }
                } else if [
                    "marketposition",
                    "entryprice",
                    "openentrymaxprofit",
                    "openentrycomission",
                    "maxpositionprofit",
                ]
                .contains(&n.as_str())
                {
                    if self.evaluate(&args[0], i)? != Some(0.0) {
                        return Err(format!("{n} 僅支援目前單一部位 (0)"));
                    }
                    if n == "openentrycomission" {
                        Some(
                            self.context_history
                                .get(i)
                                .and_then(|x| x.open_entry_commission)
                                .ok_or("OpenEntryComission 缺少現持倉進場佣金上下文")?,
                        )
                    } else if n == "openentrymaxprofit" || n == "maxpositionprofit" {
                        Some(
                            self.context_history
                                .get(i)
                                .and_then(|x| x.open_entry_max_profit)
                                .ok_or("OpenEntryMaxProfit 缺少成交路徑上下文")?,
                        )
                    } else {
                        self.vars[if n == "entryprice" {
                            "@entryprice"
                        } else {
                            "@marketposition"
                        }]
                        .get(i)
                        .copied()
                        .flatten()
                    }
                } else if [
                    "average",
                    "xaverage",
                    "stddev",
                    "highest",
                    "lowest",
                    "averagefc",
                    "highestfc",
                    "lowestfc",
                    "summation",
                ]
                .contains(&n.as_str())
                {
                    let Some(length)=self.evaluate(&args[1],i)? else {return Ok(None)};
                    let len=window_length(length)?;
                    if n == "xaverage" {
                        if i < self.bar_offset {
                            return Ok(None);
                        }
                        // One state per expression, including adaptive Length.
                        let key = (e.id, 0, i);
                        if let Some(value) = self.exponential_values.borrow().get(&key).copied() {
                            return Ok(Some(value));
                        }
                        // Series instances are independent. CurrentBar=1 seeds Price;
                        // subsequent calls preserve chronological recurrence, including
                        // conditionally requested or historical values.
                        let previous = (self.bar_offset..i).rev().find_map(|j| {
                            self.exponential_values
                                .borrow()
                                .get(&(e.id, 0, j))
                                .copied()
                                .map(|value| (j, value))
                        });
                        let (mut at, mut value) = if let Some(found) = previous {
                            found
                        } else {
                            let Some(seed) = self.evaluate(&args[0], self.bar_offset)? else {
                                return Ok(None);
                            };
                            (self.bar_offset, seed)
                        };
                        self.exponential_values
                            .borrow_mut()
                            .insert((e.id, 0, at), value);
                        while at < i {
                            at += 1;
                            let Some(price) = self.evaluate(&args[0], at)? else {
                                return Ok(None);
                            };
                            let Some(length)=self.evaluate(&args[1],at)? else {return Ok(None)};
                            // XAverage係數使用原始Length，沒有整數迴圈；本策略lencmo本來為整數。
                            if !length.is_finite() || !(1.0..=100_000.0).contains(&length) {return Err("XAverage長度須為1至100000有限數值".into());}
                            let factor=2.0/(length+1.0);
                            value = value + factor * (price - value);
                            if !value.is_finite() {
                                return Err("XAverage 非有限累計值".into());
                            }
                            self.exponential_values
                                .borrow_mut()
                                .insert((e.id, 0, at), value);
                        }
                        Some(value)
                    } else if i + 1 < len {
                        None
                    } else {
                        if n == "averagefc" {
                            // SummationFC keeps one recurrence per call site, even if
                            // NumericSimple Len changes. Len is not an instance key.
                            let key = (e.id, 0, i);
                            if let Some(sum) = self.fc_sums.borrow().get(&key).copied() {
                                return Ok(Some(sum / length));
                            }
                            // Reconstruct recurrence in chronological order even if the
                            // first invocation is conditional or a historical reference.
                            if i < self.bar_offset { return Ok(None); }
                            let previous = (self.bar_offset..i).rev().find_map(|j| {
                                self.fc_sums
                                    .borrow()
                                    .get(&(e.id, 0, j))
                                    .copied()
                                    .map(|v| (j, v))
                            });
                            let (mut at, mut sum) = if let Some((j, v)) = previous {
                                (j, v)
                            } else {
                                // CurrentBar=1 seeds newest-first from the calculation
                                // start, not from the first available warmup window.
                                // Without sufficient warmup, wait for actual history.
                                let mut seed=self.bar_offset;
                                loop {
                                    let Some(seed_length)=self.evaluate(&args[1],seed)? else {return Ok(None)};
                                    if seed+1>=window_length(seed_length)? {break;}
                                    if seed==i {return Ok(None);}
                                    seed+=1;
                                }
                                let Some(seed_length)=self.evaluate(&args[1],seed)? else {return Ok(None)};
                                let mut sum = 0.0;
                                for ago in 0..window_length(seed_length)? {
                                    let Some(v) = self.evaluate(&args[0], seed-ago)? else {
                                        return Ok(None);
                                    };
                                    sum += v;
                                }
                                (seed, sum)
                            };
                            self.fc_sums.borrow_mut().insert((e.id, 0, at), sum);
                            while at < i {
                                at += 1;
                                let Some(raw_length)=self.evaluate(&args[1],at)? else {return Ok(None)};
                                window_length(raw_length)?;
                                let lag=history_offset(raw_length)?;
                                if at<lag {return Ok(None);}
                                let (Some(new), Some(old)) = (
                                    self.evaluate(&args[0], at)?,
                                    self.evaluate(&args[0], at - lag)?,
                                ) else {
                                    return Ok(None);
                                };
                                sum = sum + new - old;
                                if !sum.is_finite() {
                                    return Err("AverageFC 非有限累計值".into());
                                }
                                self.fc_sums.borrow_mut().insert((e.id, 0, at), sum);
                            }
                            return Ok(Some(sum / length));
                        }
                        let mut values = Vec::with_capacity(len);
                        // 原廠 Summation / VariancePS 以 PriceValue[0] 起向過去累加。
                        // 浮點加法不可改變順序；例如含大值抵銷時結果會不同。
                        for j in (i + 1 - len..=i).rev() {
                            if let Some(v) = self.evaluate(&args[0], j)? {
                                values.push(v);
                            } else {
                                return Ok(None);
                            }
                        }
                        Some(match n.as_str() {
                            "summation" => values.iter().sum::<f64>(),
                            "average" => values.iter().sum::<f64>() / length,
                            "stddev" => {
                                let mean = values.iter().sum::<f64>() / length;
                                (values
                                    .iter()
                                    .map(|value| (value - mean).powi(2))
                                    .sum::<f64>()
                                    / length)
                                    .sqrt()
                            }
                            "highest" | "highestfc" => {
                                values.into_iter().fold(f64::NEG_INFINITY, f64::max)
                            }
                            _ => values.into_iter().fold(f64::INFINITY, f64::min),
                        })
                    }
                } else {
                    let x = self.evaluate(&args[0], i)?;
                    match n.as_str() {
                        "absvalue" => x.map(f64::abs),
                        "minlist" | "maxlist" => match (x, self.evaluate(&args[1], i)?) {
                            (Some(a), Some(b)) => {
                                Some(if n == "minlist" { a.min(b) } else { a.max(b) })
                            }
                            _ => None,
                        },
                        _ => return Err(format!("函式尚未支援：{n}")),
                    }
                }
            }
        };
        if value.is_some_and(|v| !v.is_finite()) {
            return Err("策略運算產生非有限數值".into());
        }
        Ok(value)
    }
    fn statements(&mut self, stmts: &[Stmt], out: &mut Vec<Order>) -> Result<(), String> {
        for s in stmts {
            match s {
                Stmt::SetExactComparison => self.comparison_epsilon = 0.0,
                // CommonHeaders/Source/FPCompareAccuracy.mh, enum value 2;
                // matches original native constructor bits 0x3d83880000000000.
                Stmt::SetMediumComparison => {
                    self.comparison_epsilon = f64::from_bits(0x3d83880000000000)
                }
                Stmt::Assign(n, e) => {
                    let v = self.evaluate(e, self.index)?;
                    self.vars.get_mut(n).unwrap()[self.index] = v;
                }
                Stmt::ArrayAssign(name, index, expression) => {
                    let index = self.array_index(name, index, self.index)?;
                    let value = self.evaluate(expression, self.index)?;
                    self.arrays.get_mut(name).unwrap()[self.index][index] = value;
                }
                Stmt::For(name, start, end, descending, body) => {
                    let integer = |n: Option<f64>| match n {
                        Some(n) if n.fract()==0.0 && n.abs()<=1_000_000.0 => Ok(n as i64),
                        _ => Err("迴圈界限須為正負1000000以內整數".to_string()),
                    };
                    let start = integer(self.evaluate(start, self.index)?)?;
                    let end = integer(self.evaluate(end, self.index)?)?;
                    self.vars.get_mut(name).unwrap()[self.index] = Some(start as f64);
                    let mut iterations = 0usize;
                    loop {
                        let current = integer(self.current_variable(name))?;
                        if if *descending { current < end } else { current > end } { break; }
                        if iterations >= 100_001 { return Err("迴圈超過100001次安全上限".into()); }
                        iterations += 1;
                        self.statements(body, out)?;
                        let current = integer(self.current_variable(name))?;
                        self.vars.get_mut(name).unwrap()[self.index] = Some((current + if *descending { -1 } else { 1 }) as f64);
                    }
                }
                Stmt::If(e, a, b) => {
                    if self.evaluate(e, self.index)?.is_some_and(|x| x != 0.0) {
                        self.statements(a, out)?;
                    } else {
                        self.statements(b, out)?;
                    }
                }
                Stmt::Order(o) => {
                    let quantity = if let Some(e) = &o.quantity {
                        match self.evaluate(e, self.index)? {
                            Some(q) if q >= 1.0 && q.fract() == 0.0 && q <= 1_000_000.0 => {
                                Some(q as i64)
                            }
                            _ => return Err("委託數量須為正整數；不支援零數量反轉".into()),
                        }
                    } else {
                        None
                    };
                    let price = if let Some(e) = &o.price {
                        match self.evaluate(e, self.index)? {
                            Some(p) => Some(p),
                            None => continue,
                        }
                    } else {
                        None
                    };
                    out.push(Order {
                        total: o.total,
                        action: o.action,
                        label: o.label.clone(),
                        quantity,
                        price,
                        kind: o.kind,
                    });
                }
            }
        }
        Ok(())
    }
    fn enter_bar(
        &mut self,
        index: usize,
        position: i64,
        entry: f64,
        contracts: i64,
    ) -> Result<(), String> {
        self.index = index;
        self.position = position;
        self.entry_price = entry;
        self.contracts = contracts;
        while self.context_history.len() < index {
            self.context_history.push(ExecutionContext::default())
        }
        if self.context_history.len() == index {
            self.context_history.push(self.execution.clone())
        } else {
            return Err("同一策略執行器不可重複或逆序執行 K 棒".into());
        }
        for (n, default) in &self.program.variables {
            let h = self.vars.get_mut(n).unwrap();
            while h.len() <= index {
                h.push(h.last().copied().unwrap_or(Some(*default)));
            }
        }
        for (name, defaults) in &self.program.arrays {
            let history = self.arrays.get_mut(name).unwrap();
            while history.len() <= index {
                history.push(history.last().cloned().unwrap_or_else(|| defaults.iter().copied().map(Some).collect()));
            }
        }
        for (n, v) in [
            ("@marketposition", position as f64),
            ("@entryprice", entry),
            ("@currentcontracts", contracts as f64),
        ] {
            let h = self.vars.entry(n.into()).or_default();
            while h.len() < index {
                h.push(Some(0.0));
            }
            h.push(Some(v));
        }
        Ok(())
    }
    pub fn step(&mut self, index: usize, position: i64, entry: f64, contracts: i64) -> Result<Vec<Order>, String> {
        self.enter_bar(index, position, entry, contracts)?;
        for (name, rt) in self.functions.borrow_mut().iter_mut() {
            rt.bar_offset = self.bar_offset;
            rt.execution = self.execution.clone();
            if self.program.simple_host_functions.contains(name) { rt.enter_bar(index, position, entry, contracts)?; }
            else { rt.step(index, position, entry, contracts)?; }
        }
        let mut orders = vec![];
        self.statements(&self.program.body, &mut orders)?;
        Ok(orders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rsi_wilder_seed_recurrence_and_var_alias() {
        let bars=numeric_bars(&[100.0,102.0,101.0,104.0,102.0,107.0]);
        let p=Program::compile("var: x(0), previous(0); x=RSI(Close,3); previous=RSI(Close,3)[1];",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=3;
        rt.step(3,0,0.0,0).unwrap();assert!((rt.current_variable("x").unwrap()-100.0*5.0/6.0).abs()<1e-12);
        rt.step(4,0,0.0,0).unwrap();assert!((rt.current_variable("x").unwrap()-100.0*5.0/9.0).abs()<1e-12);
        rt.step(5,0,0.0,0).unwrap();assert!((rt.current_variable("x").unwrap()-100.0*65.0/81.0).abs()<1e-12);
        assert!((rt.current_variable("previous").unwrap()-100.0*5.0/9.0).abs()<1e-12);
        let p=Program::compile("Vars: x(0); x=RSI(Close,2.5);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=3;assert!(rt.step(3,0,0.0,0).is_err());
        let flat=numeric_bars(&[100.0,100.0,100.0]);let p=Program::compile("var: x(0); x=RSI(Close,2);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&flat);rt.bar_offset=2;assert!(rt.step(2,0,0.0,0).unwrap_err().contains("不代填"));
    }
    #[test]
    fn workbench_user_rsi_source_compiles_without_rewriting_orders() {
        let source=include_str!("../tests/fixtures/pl-workbench/rsi-strategy.pl");
        let p=Program::compile(source,&BTreeMap::new()).unwrap();
        let bars=numeric_bars(&(0..80).map(|i|100.0+i as f64).collect::<Vec<_>>());
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=50;
        let orders=rt.step(50,0,0.0,0).unwrap();
        // 使用者 mp>=-1 在空手時也會產生 BuyToCover；保留此條件，
        // 撮合器依實際部位處理，而不是在解析器擅自刪除委託。
        assert_eq!(orders.len(),2);assert_eq!(orders[0].kind,Kind::Stop);
        assert_eq!(orders[0].price,Some(bars[50].high+2.0));
        assert_eq!(orders[1].kind,Kind::Market);
    }
    #[test]
    fn nh14_fractional_history_rounds_nearest_but_window_length_truncates() {
        let bars=numeric_bars(&[100.0,200.0,90.0,40.0]);
        let p=Program::compile("Vars: near_low(0), half(0), integer(0), hi(0), sum(0), avg(0); near_low=Close[1.49]; half=Close[1.5]; integer=Close[1]; hi=Highest(Close,2.9); sum=Summation(Close,2.9); avg=Average(Close,2.9);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.step(3,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("near_low"),Some(90.0));assert_eq!(rt.current_variable("half"),Some(200.0));assert_eq!(rt.current_variable("integer"),Some(90.0));
        assert_eq!(rt.current_variable("hi"),Some(90.0));assert_eq!(rt.current_variable("sum"),Some(130.0));assert_eq!(rt.current_variable("avg"),Some(130.0/2.9));
        for n in 0..=100 {assert_eq!(history_offset(n as f64).unwrap(),n);}
        assert_eq!(history_offset(2.5).unwrap(),3);
        for n in [-0.1,f64::NAN,f64::INFINITY,1_000_001.0] {assert!(history_offset(n).is_err());}
        for n in [0.9,f64::NAN,f64::INFINITY,100_001.0] {assert!(window_length(n).is_err());}
    }
    #[test]
    fn native_window_functions_keep_newest_first_sum_and_raw_fractional_divisor() {
        // Original PL receipts: outputs/mc-window-functions-native-v6/evidence.json.
        // Reverse order is observable under cancellation; truncating the divisor
        // is independently distinguishable from truncating only the loop count.
        let bars=numeric_bars(&[1e16,-1e16,1.0]);
        let p=Program::compile("Vars: total(0), mean(0); total=Summation(Close,3); mean=Average(Close,3);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=2;rt.step(2,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("total"),Some(0.0));
        assert_eq!(rt.current_variable("mean"),Some(0.0));
        let bars=numeric_bars(&[1.0,3.0,5.0]);
        let p=Program::compile("Vars: mean(0), deviation(0), atr(0); mean=Average(Close,2.5); deviation=StdDev(Close,2.5); atr=AvgTrueRange(2.5);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=2;rt.step(2,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("mean"),Some(3.2));
        assert!((rt.current_variable("deviation").unwrap()-1.312_f64.sqrt()).abs()<1e-14);
        assert_eq!(rt.current_variable("atr"),Some(16.0));
    }
    #[test]
    fn averagefc_seeds_at_calculation_start_and_keeps_one_changing_length_state() {
        let bars=numeric_bars(&[10.0,20.0,30.0,40.0,50.0,60.0]);
        let p=Program::compile("Vars: n(2), adaptive(0), fractional(0), skipped(0), previous(0); if CurrentBar=2 then n=3; adaptive=AverageFC(Close,n); fractional=AverageFC(Close,2.9); if CurrentBar>=3 then skipped=AverageFC(Close,2.9); previous=AverageFC(Close,2.9)[1];",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=3;
        rt.step(3,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("adaptive"),Some(35.0));
        assert_eq!(rt.current_variable("fractional"),Some(70.0/2.9));
        assert_eq!(rt.current_variable("previous"),None);
        rt.step(4,0,0.0,0).unwrap();
        // Original SummationFC: prior sum70 + Price50 - Price[3]=20.
        assert_eq!(rt.current_variable("adaptive"),Some(100.0/3.0));
        assert_eq!(rt.current_variable("fractional"),Some(100.0/2.9));
        rt.step(5,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("fractional"),Some(130.0/2.9));
        assert_eq!(rt.current_variable("skipped"),Some(130.0/2.9));
        assert_eq!(rt.current_variable("previous"),Some(100.0/2.9));
    }
    #[test]
    fn window_builtin_matches_shipped_pl_replay_with_warmup_and_length_changes() {
        // Preserve shipped PL verbatim in fixtures; rename only function symbols
        // to execute its loop/recurrence independently of the native shortcuts.
        let sources=BTreeMap::from([
            ("originalsum".into(),include_str!("../tests/fixtures/mc-window-functions/Summation.pl").replace("Summation", "OriginalSum")),
            ("originalsumfc".into(),include_str!("../tests/fixtures/mc-window-functions/SummationFC.pl").replace("SummationFC", "OriginalSumFC")),
            ("originalaveragefc".into(),include_str!("../tests/fixtures/mc-window-functions/AverageFC.pl").replace("AverageFC", "OriginalAverageFC").replace("SummationFC", "OriginalSumFC")),
        ]);
        let bars=numeric_bars(&[100.0,90.0,105.0,120.0,95.0,80.0,130.0]);
        let p=Program::with_functions("Vars: n(2), a(0), b(0), total(0), reference(0); if CurrentBar=2 then n=3; if CurrentBar=3 then n=1; a=AverageFC(Close,n); b=OriginalAverageFC(Close,n); total=Summation(Close,n); reference=OriginalSum(Close,n);",&BTreeMap::new(),&sources).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=3;
        for i in 3..bars.len() {
            rt.step(i,0,0.0,0).unwrap();
            assert_eq!(rt.current_variable("a"),rt.current_variable("b"),"AverageFC bar{i}");
            assert_eq!(rt.current_variable("total"),rt.current_variable("reference"),"Summation bar{i}");
        }
    }
    #[test]
    fn nested_functions_resolve_in_dependency_order_and_reject_ambiguous_graphs() {
        // Caller sorts before callee; filenames/map order must not affect compile.
        let functions=BTreeMap::from([
            ("aaa".into(),"Inputs: p(NumericSeries); aaa=zzz(p)+1;".into()),
            ("zzz".into(),"Inputs: p(NumericSeries); zzz=p*2;".into()),
        ]);
        let p=Program::with_functions("Vars: a(0), b(0); a=aaa(Close); b=aaa(High);",&BTreeMap::new(),&functions).unwrap();
        let bars=numeric_bars(&[10.0,20.0]);let mut rt=Runtime::new(&p,&bars);
        for i in 0..bars.len() {rt.step(i,0,0.0,0).unwrap();}
        assert_eq!(rt.current_variable("a"),Some(41.0));assert_eq!(rt.current_variable("b"),Some(61.0));
        let cycle=BTreeMap::from([("a".into(),"a=b;".into()),("b".into(),"b=a;".into())]);
        assert!(Program::with_functions("",&BTreeMap::new(),&cycle).unwrap_err().contains("循環相依"));
        let recursion=BTreeMap::from([("f".into(),"Inputs: p(NumericSeries); f=f(p);".into())]);
        assert!(Program::with_functions("",&BTreeMap::new(),&recursion).unwrap_err().contains("遞迴"));
        let duplicate=BTreeMap::from([("A".into(),"A=1;".into()),("a".into(),"a=2;".into())]);
        assert!(Program::with_functions("",&BTreeMap::new(),&duplicate).unwrap_err().contains("重複"));
        let missing=BTreeMap::from([("a".into(),"a=missing(1);".into())]);
        assert!(Program::with_functions("",&BTreeMap::new(),&missing).unwrap_err().contains("缺少自訂函式"));
        let comments=BTreeMap::from([("a".into(),"{ b is not a dependency } a=1;".into()),("b".into(),"b=a+1;".into())]);
        assert!(Program::with_functions("",&BTreeMap::new(),&comments).is_ok());
    }
    #[test]
    fn warmup_tick_history_uses_explicit_trade_volume_without_running_strategy() {
        let mut bars=numeric_bars(&[100.0,200.0,90.0,40.0]);
        for (i,b) in bars.iter_mut().enumerate() {b.volume=(i+1) as f64*10.0;}
        let p=Program::compile("Vars: n(0), avg(0); n=n+1; avg=Average(Ticks[1],3);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=3;rt.execution.ticks=Some(40.0);rt.step(3,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("n"),Some(1.0));assert_eq!(rt.current_variable("avg"),Some(20.0));
        let mut unspecified=Runtime::new(&p,&bars);unspecified.bar_offset=3;
        assert!(unspecified.step(3,0,0.0,0).unwrap_err().contains("Build Volume"));
    }
    #[test]
    fn native_simple_values_remain_current_inside_historical_price_expression() {
        let bars=numeric_bars(&[10.0,100.0,30.0,40.0,50.0]);
        let source="Vars: n(1), highest_shifted(0), previous_n(0); if CurrentBar=5 then n=2 else n=1; highest_shifted=Highest(Close[n],2); previous_n=n[1];";
        let mut simple=Program::compile(source,&BTreeMap::new()).unwrap();
        simple.set_simple_variables(&["n".into()]).unwrap();
        let series=Program::compile(source,&BTreeMap::new()).unwrap();
        let mut a=Runtime::new(&simple,&bars);let mut b=Runtime::new(&series,&bars);
        for i in 0..5 {a.step(i,0,0.0,0).unwrap();b.step(i,0,0.0,0).unwrap();}
        assert_eq!(a.current_variable("previous_n"),Some(2.0));
        assert_eq!(b.current_variable("previous_n"),Some(1.0));
        // 當棒n=2：Close[2]在回看兩棒時分別取30、100；Series舊n=1會漏掉100。
        assert_eq!(a.current_variable("highest_shifted"),Some(100.0));
        assert_eq!(b.current_variable("highest_shifted"),Some(30.0));
        assert!(simple.set_simple_variables(&["unknown".into()]).is_err());
    }
    #[test]
    fn nh14_array_shift_inclusive_bound_and_scalar_history_remain_distinct() {
        let bars=numeric_bars(&[10.0,20.0,30.0]);
        let p=Program::compile("Vars: j(0), total(0), prior(0), edge(0); Arrays: adata[20](0); if CurrentBar=1 then adata[20]=99; for j=19 downto 1 begin adata[j]=adata[j-1]; end; adata[0]=Close; total=0; for j=0 to 19 begin total=total+adata[j]; end; prior=total[1]; edge=adata[20];",&BTreeMap::new()).unwrap();
        assert_eq!(p.arrays["adata"].len(),21);
        let mut rt=Runtime::new(&p,&bars);
        for i in 0..3 {rt.step(i,0,0.0,0).unwrap();}
        assert_eq!(rt.current_variable("total"),Some(60.0));
        assert_eq!(rt.current_variable("prior"),Some(30.0));
        assert_eq!(rt.current_variable("edge"),Some(99.0));
        assert_eq!(rt.arrays["adata"][2][..3],[Some(30.0),Some(20.0),Some(10.0)]);
    }
    #[test]
    fn array_invalid_indices_and_undeclared_loop_variables_fail_explicitly() {
        let bars=numeric_bars(&[1.0]);
        for source in ["Arrays: a[2](0); a[3]=1;","Vars: x(0); Arrays: a[2](0); x=a[-1];","Vars: x(0); Arrays: a[2](0); x=a[0.5];"] {
            let p=Program::compile(source,&BTreeMap::new()).unwrap();
            assert!(Runtime::new(&p,&bars).step(0,0,0.0,0).unwrap_err().contains("索引"));
        }
        assert!(Program::compile("Arrays: a[2](0); for j=0 to 2 begin a[j]=1; end;",&BTreeMap::new()).is_err());
    }
    #[test]
    fn nh14_swing_tie_direction_extrema_count_sum_and_variadic_lists() {
        let bars=numeric_bars(&[1.0,5.0,5.0,2.0,1.0]);
        let p=Program::compile("Vars: swing(0), hi(0), lo(0), count(0), total(0), minimum(0), maximum(0), integer(0), powered(0); swing=SwingHigh(1,Close,1,4); hi=HighestBar(Close,4); lo=LowestBar(Close,4); count=CountIf(Close>=5,4); total=Summation(Close,4); minimum=MinList(10,7,3,8); maximum=MaxList(10,7,3,8); integer=IntPortion(-2.7); powered=Power(3,2);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.step(4,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("swing"),Some(5.0));
        assert_eq!(rt.current_variable("hi"),Some(2.0));
        assert_eq!(rt.current_variable("lo"),Some(0.0));
        assert_eq!(rt.current_variable("count"),Some(2.0));
        assert_eq!(rt.current_variable("total"),Some(13.0));
        assert_eq!(rt.current_variable("minimum"),Some(3.0));assert_eq!(rt.current_variable("maximum"),Some(10.0));
        assert_eq!(rt.current_variable("integer"),Some(-2.0));assert_eq!(rt.current_variable("powered"),Some(9.0));
    }
    #[test]
    fn nh14_obv_and_trade_context_history_have_explicit_values() {
        let bars=numeric_bars(&[10.0,12.0,12.0,8.0]);
        let p=Program::compile("Vars: value(0), exit(0), age(0); value=OBV; exit=ExitPrice(1); age=BarsSinceEntry(0);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);
        for i in 0..4 {rt.execution.ticks=Some(10.0*(i+1) as f64);rt.execution.entry_bar=Some(1);rt.execution.recent_exit_prices=vec![123.0];rt.step(i,0,0.0,0).unwrap();}
        assert_eq!(rt.current_variable("value"),Some(-20.0));assert_eq!(rt.current_variable("exit"),Some(123.0));assert_eq!(rt.current_variable("age"),Some(2.0));
    }
    #[test]
    fn nh14_fix2_original_source_and_dependencies_compile_without_rewrites() {
        let functions=BTreeMap::from([
            ("rea_nh14_fnam".into(),include_str!("../../strategies/nh14-recovery/delivery-length-fix-v2/f_REA_NH14_FNAM.pl").into()),
            ("rea_nh14_mc_s16".into(),include_str!("../../strategies/nh14-recovery/delivery-length-fix-v2/f_REA_NH14_MC_S16.pl").into()),
            ("rea_nh14_ld".into(),include_str!("../../strategies/nh14-recovery/delivery-length-fix-v2/f_REA_NH14_LD.pl").into()),
            ("rea_nh14_kgi_id".into(),include_str!("../../strategies/nh14-recovery/delivery-length-fix-v2/f_REA_NH14_KGI_ID.pl").into()),
        ]);
        let p=Program::with_functions(include_str!("../../strategies/nh14-recovery/delivery-length-fix-v2/s_REA_KGI_07_NH14_Fix2.pl"),&BTreeMap::new(),&functions).unwrap();
        assert_eq!(p.inputs.len(),36);assert_eq!(p.functions.len(),4);assert_eq!(p.arrays["adata"].len(),21);
    }
    #[test]
    fn linear_reg_slope_preserves_native_sign_and_fractional_length_algebra() {
        let bars = numeric_bars(&[1.0, 5.0, 9.0]);
        let p = Program::compile("Vars: rising(0), falling(0), fractional(0); rising=LinearRegSlope(Close,2); falling=LinearRegSlope(-Close,3); fractional=LinearRegSlope(Close,2.5);", &BTreeMap::new()).unwrap();
        let mut rt = Runtime::new(&p, &bars);rt.bar_offset=2;
        rt.step(2,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("rising"),Some(4.0));
        assert_eq!(rt.current_variable("falling"),Some(-4.0));
        // Native PL retains raw Len=2.5 in its denominator while iterating two lags.
        assert!((rt.current_variable("fractional").unwrap()-5.0285714285714285).abs()<1e-12);
        assert!(Program::compile("Vars: a(0); a=LinearRegSlope(Close);",&BTreeMap::new()).is_err());
    }
    #[test]
    fn linear_reg_slope_dynamic_simple_length_retains_previous_value_per_call_site() {
        let bars = numeric_bars(&[10.0,20.0,40.0,100.0,99.0,98.0]);
        let p=Program::compile("Vars: n(3), a(0), b(0), initial(0), prior(0); if CurrentBar=2 then n=2; if CurrentBar=3 then n=1; if CurrentBar=4 then n=-1; a=LinearRegSlope(Close,n); b=LinearRegSlope(-Close,3); initial=LinearRegSlope(Close,0); prior=LinearRegSlope(Close,3)[1];", &BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=2;
        for (i,expected) in [(2,15.0),(3,60.0),(4,60.0),(5,60.0)] {
            rt.step(i,0,0.0,0).unwrap();
            assert_eq!(rt.current_variable("a"),Some(expected));
            assert_eq!(rt.current_variable("initial"),Some(0.0));
            if i==2 {assert_eq!(rt.current_variable("prior"),Some(0.0));}
        }
        assert_eq!(rt.current_variable("b"),Some(1.0));
        assert_eq!(rt.current_variable("prior"),Some(29.5));
    }
    #[test]
    fn linear_reg_slope_reads_warmup_initial_values_and_freezes_simple_price_expressions() {
        let bars=numeric_bars(&[10.0,20.0,40.0]);
        let source="Vars: sample(0), slope(0), market(0); sample=Close; slope=LinearRegSlope(sample,3); market=LinearRegSlope(Close,3);";
        let series=Program::compile(source,&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&series,&bars);rt.bar_offset=2;rt.step(2,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("slope"),Some(20.0)); // sample warmup is its own declared zero.
        assert_eq!(rt.current_variable("market"),Some(15.0));
        let mut simple=series.clone();simple.set_simple_variables(&["sample".into()]).unwrap();
        let mut rt=Runtime::new(&simple,&bars);rt.bar_offset=2;rt.step(2,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("slope"),Some(0.0)); // current Simple=40 at every lag.
        let missing=Program::compile("Vars: slope(0); slope=LinearRegSlope(Close,3);",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&missing,&bars);rt.step(0,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("slope"),None); // Never invent unavailable price history.
    }
    #[test]
    fn yao1_typed_series_function_keeps_arguments_and_call_sites_independent() {
        let bars = numeric_bars(&[10.0, 20.0, 30.0, 40.0, 50.0]);
        let f = BTreeMap::from([("norm".into(), "Inputs: Value(NumericSeries), Period(NumericSimple); Vars: hi(0), lo(0); hi=Highest(Value,Period); lo=Lowest(Value,Period); if hi=lo then norm=0 else norm=(Value-lo)/(hi-lo);".into())]);
        let p=Program::with_functions("Vars: a(0), b(0); a=norm(Close,3); b=norm(-Close,2);", &BTreeMap::new(), &f).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=2;
        for i in 2..5 { rt.step(i,0,0.0,0).unwrap();assert_eq!(rt.current_variable("a"),Some(1.0));assert_eq!(rt.current_variable("b"),Some(0.0)); }
        assert!(Program::with_functions("Vars: a(0); a=norm(Close);",&BTreeMap::new(),&f).is_err());
    }
    #[test]
    fn yao1_adaptive_xaverage_uses_actual_previous_state_when_length_changes() {
        let bars=numeric_bars(&[100.0,110.0,120.0,130.0]);
        let p=Program::compile("Vars: n(3), a(0), previous(0); if CurrentBar=2 then n=9 else n=3; a=XAverage(Close,n); previous=a[1];",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);
        let expected=[100.0,102.0,111.0,120.5];
        for (i,value) in expected.into_iter().enumerate() {rt.step(i,0,0.0,0).unwrap();assert_eq!(rt.current_variable("a"),Some(value));}
        assert_eq!(rt.current_variable("previous"),Some(111.0));
    }
    #[test]
    fn yao1_exit_names_are_literal_and_bar_counts_include_exit_bar_zero() {
        let bars=numeric_bars(&[100.0,101.0,102.0]);
        let p=Program::compile("Vars: name(false), wild(false), count(0), net(0); name=ExitName(1)=\"s%\"; wild=ExitName(1)=\"SoftMaxLoss_S\"; count=BarsSinceExit(1); net=NetProfit;",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.execution.net_profit=Some(0.0);
        rt.step(0,0,0.0,0).unwrap();assert_eq!(rt.current_variable("count"),Some(-1.0));
        rt.execution.net_profit=Some(-1600.0);rt.execution.recent_exits=vec![(1,"SoftMaxLoss_S".into())];
        rt.step(1,0,0.0,0).unwrap();assert_eq!(rt.current_variable("count"),Some(0.0));assert_eq!(rt.current_variable("name"),Some(0.0));assert_eq!(rt.current_variable("wild"),Some(1.0));assert_eq!(rt.current_variable("net"),Some(-1600.0));
        rt.step(2,0,0.0,0).unwrap();assert_eq!(rt.current_variable("count"),Some(1.0));
    }
    #[test]
    fn yao1_cross_round_atr_and_selected_iff_branch() {
        let bars=numeric_bars(&[100.0,100.0,110.0,100.0]);
        let p=Program::compile("Vars: up(false), down(false), r(0), atr(0), safe(0); up=Close crosses above 100; down=Close crosses below 105; r=Round(-2.5,0); atr=AvgTrueRange(2); safe=IFF(Close=100,7,1/(Close-100));",&BTreeMap::new()).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.bar_offset=2;
        rt.step(2,0,0.0,0).unwrap();assert_eq!(rt.current_variable("up"),Some(1.0));assert_eq!(rt.current_variable("r"),Some(-3.0));assert_eq!(rt.current_variable("atr"),Some(20.0));
        rt.step(3,0,0.0,0).unwrap();assert_eq!(rt.current_variable("down"),Some(1.0));assert_eq!(rt.current_variable("safe"),Some(7.0));
    }
    #[test]
    fn yao1_fix1_source_compiles_without_replacing_functions_or_logic() {
        let functions=BTreeMap::from([
            ("rea_yao1_df".into(),include_str!("../../strategies/yao1-recovery/candidate-storage-fix-v1/f_REA_YAO1_DF.pl").into()),
            ("rea_yao1_mc_s16".into(),include_str!("../../strategies/yao1-recovery/candidate-storage-fix-v1/f_REA_YAO1_MC_S16.pl").into()),
            ("rea_yao1_ld".into(),include_str!("../../strategies/yao1-recovery/candidate-storage-fix-v1/f_REA_YAO1_LD.pl").into()),
            ("rea_yao1_kgi_id".into(),include_str!("../../strategies/yao1-recovery/candidate-storage-fix-v1/f_REA_YAO1_KGI_ID.pl").into()),
        ]);
        let p=Program::with_functions(include_str!("../../strategies/yao1-recovery/candidate-storage-fix-v1/s_REA_KGI_07_YAO1_Fix1.pl"),&BTreeMap::new(),&functions).unwrap();
        assert_eq!(p.inputs.len(),47);assert_eq!(p.functions.len(),4);
    }
    fn numeric_bars(prices: &[f64]) -> Vec<Candle> {
        prices
            .iter()
            .enumerate()
            .map(|(index, price)| Candle {
                datetime: format!("2026-10-01T09:{index:02}:00"),
                open: *price,
                high: price + 10.0,
                low: price - 10.0,
                close: *price,
                volume: 1.0,
                pl_date: 1261001,
                pl_time: 900 + index as u32,
                source_rows: 1,
            })
            .collect()
    }
    #[test]
    fn medium_comparison_matches_absolute_epsilon_and_can_switch_back_to_exact() {
        let source = "Vars: eq(0), ne(0), lt(0), gt(0), le(0), ge(0), far(0), exact(0), boundary(0); SetFPCompareAccuracy(2); eq=0=0.000000000001; ne=0<>0.000000000001; lt=0<0.000000000001; gt=0.000000000001>0; le=0.000000000001<=0; ge=0>=0.000000000001; far=0<0.000000000004; boundary=0=0.0000000000022204460492503131; SetFPCompareAccuracy(FPCExact); exact=0=0.000000000001;";
        let bars = numeric_bars(&[100.0]);
        let program = Program::compile(source, &BTreeMap::new()).unwrap();
        let mut runtime = Runtime::new(&program, &bars);
        runtime.step(0, 0, 0.0, 0).unwrap();
        for name in ["eq", "le", "ge", "far", "boundary"] {
            assert_eq!(runtime.current_variable(name), Some(1.0), "{name}");
        }
        for name in ["ne", "lt", "gt", "exact"] {
            assert_eq!(runtime.current_variable(name), Some(0.0), "{name}");
        }
        assert!(
            Program::compile("SetFPCompareAccuracy(FPCMedAccuracy);", &BTreeMap::new()).is_ok()
        );
        assert_eq!(f64::from_bits(0x3d83880000000000), 2.2204460492503131e-12);
    }
    #[test]
    fn xaverage_seeds_currentbar_one_and_preserves_independent_series_and_history() {
        let bars = numeric_bars(&[1.0, 99.0, 1000.0, 1004.0, 1008.0]);
        let program = Program::compile("Vars: a(0), b(0), conditional(0), prior(0); a=XAverage(Close,3); b=XAverage(High,3); if CurrentBar>=3 then conditional=XAverage(Close,3); prior=XAverage(Close,3)[1];", &BTreeMap::new()).unwrap();
        let mut runtime = Runtime::new(&program, &bars);
        runtime.bar_offset = 2;
        runtime.step(2, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("a"), Some(1000.0));
        assert_eq!(runtime.current_variable("prior"), None);
        runtime.step(3, 0, 0.0, 0).unwrap();
        runtime.step(4, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("a"), Some(1005.0));
        assert_eq!(runtime.current_variable("b"), Some(1015.0));
        assert_eq!(runtime.current_variable("conditional"), Some(1005.0));
        assert_eq!(runtime.current_variable("prior"), Some(1002.0));
    }
    #[test]
    fn true_range_uses_previous_close_and_stddev_is_population_not_sample() {
        let mut bars = numeric_bars(&[100.0, 110.0, 95.0]);
        bars[1].high = 113.0;
        bars[1].low = 108.0;
        bars[2].high = 97.0;
        bars[2].low = 94.0;
        let program = Program::compile(
            "Vars: tr(0), deviation(0); tr=TrueRange; deviation=StdDev(Close,2);",
            &BTreeMap::new(),
        )
        .unwrap();
        let mut runtime = Runtime::new(&program, &bars);
        runtime.step(0, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("tr"), None);
        runtime.step(1, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("tr"), Some(13.0));
        assert_eq!(runtime.current_variable("deviation"), Some(5.0));
        runtime.step(2, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("tr"), Some(16.0));
        assert_eq!(runtime.current_variable("deviation"), Some(7.5));
        assert!(Program::compile("Vars: a(0); a=StdDev(Close,2,1);", &BTreeMap::new()).is_err());
    }
    #[test]
    fn open_entry_commission_requires_actual_context_and_retains_bar_history() {
        let bars = numeric_bars(&[100.0, 101.0]);
        let program = Program::compile("Vars: fee(0), previous(0); fee=OpenEntryComission(0); previous=OpenEntryComission(0)[1];", &BTreeMap::new()).unwrap();
        let mut missing = Runtime::new(&program, &bars);
        assert!(missing
            .step(0, 1, 100.0, 2)
            .unwrap_err()
            .contains("OpenEntryComission"));
        let mut runtime = Runtime::new(&program, &bars);
        runtime.execution.open_entry_commission = Some(1600.0);
        runtime.step(0, 1, 100.0, 2).unwrap();
        runtime.execution.open_entry_commission = Some(0.0);
        runtime.step(1, 0, 0.0, 0).unwrap();
        assert_eq!(runtime.current_variable("fee"), Some(0.0));
        assert_eq!(runtime.current_variable("previous"), Some(1600.0));
    }
    #[test]
    fn corrected_ma5_weighted_cost_reset_and_ticks_mode() {
        let source="Vars: sessionchange(false), totalp(0), totalv(0), cost(0), continecost(0), m(0); sessionchange=SessionLastBar; if sessionchange[1] then begin totalp=Close*Ticks; totalv=Ticks; end else begin totalp=totalp+Close*Ticks; totalv=totalv+Ticks; end; if totalv<>0 then cost=totalp/totalv else cost=0; if sessionchange[1] then begin cost=Close;continecost=0;end else continecost=continecost+Close-cost; m=MaxPositionProfit(0)/BigPointValue;";
        let bars: Vec<_> = [100.0, 102.0, 98.0, 110.0, 120.0]
            .into_iter()
            .enumerate()
            .map(|(i, p)| Candle {
                datetime: format!("2026-10-01T{:02}:00:00", i),
                open: p,
                high: p,
                low: p,
                close: p,
                volume: 10.0,
                pl_date: 1261001,
                pl_time: i as u32 * 100,
                source_rows: 1,
            })
            .collect();
        let p = Program::compile(source, &BTreeMap::new()).unwrap();
        let mut rt = Runtime::new(&p, &bars);
        for i in 0..5 {
            rt.execution = ExecutionContext {
                ticks: Some(if i == 4 { 0.0 } else { 10.0 }),
                big_point_value: Some(200.0),
                session_last_bar: Some(i == 2),
                open_entry_max_profit: Some(400.0),
                ..Default::default()
            };
            rt.step(i, 0, 0.0, 0).unwrap();
            assert_eq!(
                rt.current_variable("continecost"),
                Some([0.0, 1.0, -1.0, 0.0, 10.0][i])
            );
            assert_eq!(rt.current_variable("m"), Some(2.0));
        }
        let mut missing = Runtime::new(&p, &bars);
        missing.execution.session_last_bar = Some(false);
        assert!(missing.step(0, 0, 0.0, 0).unwrap_err().contains("Ticks"));
    }
    #[test]
    fn ma5_numeric_functions_and_exact_comparison() {
        let mut c = vec![];
        for (i, close) in [101.0, 103.0, 105.0].into_iter().enumerate() {
            c.push(Candle {
                datetime: format!("2026-10-{:02}", i + 1),
                open: close,
                high: close + 2.0,
                low: close - 2.0,
                close,
                volume: 1.0,
                pl_date: 1261001 + i as i32,
                pl_time: 0,
                source_rows: 1,
            });
        }
        let p=Program::compile("Vars: avg(0), hi(0), lo(0), price(0), choice(0); SetFPCompareAccuracy(FPCExact); avg=AverageFC(Close,2); hi=HighestFC(High,2); lo=LowestFC(Low,2); price=EntryPrice(0); choice=IFF(Close>102,7,-7);",&BTreeMap::new()).unwrap();
        let mut rt = Runtime::new(&p, &c);
        rt.step(0, 1, 99.0, 1).unwrap();
        assert_eq!(rt.current_variable("choice"), Some(-7.0));
        rt.step(1, 1, 99.0, 1).unwrap();
        assert_eq!(rt.current_variable("avg"), Some(102.0));
        assert_eq!(rt.current_variable("hi"), Some(105.0));
        assert_eq!(rt.current_variable("lo"), Some(99.0));
        rt.step(2, 1, 99.0, 1).unwrap();
        assert_eq!(rt.current_variable("avg"), Some(104.0));
        assert_eq!(rt.current_variable("price"), Some(99.0));
        assert_eq!(rt.current_variable("choice"), Some(7.0));
        assert!(
            Program::compile("SetFPCompareAccuracy(FPCLowAccuracy);", &BTreeMap::new()).is_err()
        );
    }
    #[test]
    fn ma5_context_preserves_history_and_rejects_missing_session() {
        let c = [
            Candle {
                datetime: "2026-10-01".into(),
                open: 100.0,
                high: 110.0,
                low: 90.0,
                close: 100.0,
                volume: 1.0,
                pl_date: 1261001,
                pl_time: 1345,
                source_rows: 1,
            },
            Candle {
                datetime: "2026-10-02".into(),
                open: 100.0,
                high: 110.0,
                low: 90.0,
                close: 100.0,
                volume: 1.0,
                pl_date: 1261002,
                pl_time: 1345,
                source_rows: 1,
            },
        ];
        let p=Program::compile("Vars: n(0), previous(0), m(0), s(false); n=EntriesToday(Date); previous=EntriesToday(Date)[1]; m=OpenEntryMaxProfit(0); s=SessionLastBar;",&BTreeMap::new()).unwrap();
        let mut rt = Runtime::new(&p, &c);
        rt.execution = ExecutionContext {
            ticks: None,
            big_point_value: None,
            session_last_bar: Some(true),
            recent_entry_dates: vec![1261001; 12],
            open_entry_max_profit: Some(200.0),
            open_entry_commission: None,
            ..Default::default()
        };
        rt.step(0, 1, 100.0, 1).unwrap();
        assert_eq!(rt.current_variable("n"), Some(10.0));
        rt.execution = ExecutionContext {
            ticks: None,
            big_point_value: None,
            session_last_bar: Some(false),
            recent_entry_dates: vec![1261001, 1261002],
            open_entry_max_profit: Some(300.0),
            open_entry_commission: None,
            ..Default::default()
        };
        rt.step(1, 1, 100.0, 1).unwrap();
        assert_eq!(rt.current_variable("previous"), Some(10.0));
        assert_eq!(rt.current_variable("n"), Some(1.0));
        assert_eq!(rt.current_variable("m"), Some(300.0));
        let mut missing = Runtime::new(&p, &c);
        assert!(missing.step(0, 0, 0.0, 0).is_err());
        let bad = BTreeMap::from([("close".into(), "close = 1;".into())]);
        assert!(Program::with_functions("", &BTreeMap::new(), &bad).is_err());
    }
    #[test]
    fn missing_functions_are_never_substituted() {
        let e=Program::compile("if _01_KGI_ID and _01_ld then Buy next bar at market; if _01_mc = 1 then Sell next bar at market;",&BTreeMap::new()).unwrap_err();
        for n in ["_01_kgi_id", "_01_ld", "_01_mc"] {
            assert!(e.contains(n));
        }
    }
    #[test]
    fn rejects_unsupported_and_future() {
        assert!(Program::compile(
            "for a = 1 to 10 begin Buy next bar at market; end;",
            &BTreeMap::new()
        )
        .is_err());
        assert!(Program::compile(
            "if Secret(Close) > 0 then Buy next bar at market;",
            &BTreeMap::new()
        )
        .is_err());
    }
    #[test]
    fn parses_history_orders_and_labels() {
        let p=Program::compile("Inputs: n(20); Vars: a(0); if Average(Close,n)[1] > Lowest(Low,20)[1] then begin a = -2; Buy (\"BB \" ) next bar at Highest(High,1)+3 stop; end; Sell (\"exit\") this bar on close;",&BTreeMap::new()).unwrap();
        assert_eq!(p.body.len(), 2);
    }
    fn sample(date: i32, time: u32) -> Candle {
        Candle {
            datetime: format!("{date}-{time}"),
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
            pl_date: date,
            pl_time: time,
            source_rows: 1,
        }
    }
    #[test]
    fn calendar_function_persists_within_date_then_resets() {
        let mut functions = BTreeMap::new();
        functions.insert(
            "_01_ld".into(),
            include_str!("../../strategies/functions/_01_ld.pl").into(),
        );
        let program = Program::with_functions(
            "if _01_ld then Buy next bar at market;",
            &BTreeMap::new(),
            &functions,
        )
        .unwrap();
        let bars = vec![
            sample(1261020, 1000),
            sample(1261021, 1000),
            sample(1261021, 1300),
            sample(1261022, 1000),
        ];
        let mut rt = Runtime::new(&program, &bars);
        assert_eq!(rt.step(0, 0, 0.0, 0).unwrap().len(), 0);
        assert_eq!(rt.step(1, 0, 0.0, 0).unwrap().len(), 1);
        assert_eq!(rt.step(2, 0, 0.0, 0).unwrap().len(), 1);
        assert_eq!(rt.step(3, 0, 0.0, 0).unwrap().len(), 0);
    }
    #[test]
    fn custom_function_history_tracks_contracts() {
        let mut f = BTreeMap::new();
        f.insert(
            "_01_mc".into(),
            "_01_mc = MarketPosition(0) * CurrentContracts;".into(),
        );
        let p = Program::with_functions(
            "if _01_mc[1] = 2 and _01_mc = -3 then Buy next bar at market;",
            &BTreeMap::new(),
            &f,
        )
        .unwrap();
        let bars = vec![sample(1261021, 1000), sample(1261021, 1100)];
        let mut rt = Runtime::new(&p, &bars);
        assert!(rt.step(0, 1, 100.0, 2).unwrap().is_empty());
        assert_eq!(rt.step(1, -1, 100.0, 3).unwrap().len(), 1);
    }
    #[test]
    fn simple_host_function_history_recomputes_current_position_without_rewriting_strategy() {
        let functions = BTreeMap::from([("mp".into(), "mp=MarketPosition(0)*CurrentContracts;".into())]);
        let source="Vars: saved(0), inherited(0), changed(0); saved=mp; inherited=mp[1]; if mp<>mp[1] then changed=changed+1;";
        let bars=vec![sample(1261021,1000),sample(1261021,1100),sample(1261021,1200)];
        for storage in [16,32] {
            let mut p=Program::with_functions(source,&BTreeMap::new(),&functions).unwrap();
            p.set_function_storage_types(&BTreeMap::from([("MP".into(),storage)])).unwrap();
            let mut rt=Runtime::new(&p,&bars);
            rt.step(0,0,0.0,0).unwrap();rt.step(1,1,100.0,2).unwrap();
            assert_eq!(rt.current_variable("saved"),Some(2.0));
            assert_eq!(rt.current_variable("inherited"),Some(if storage==16 {2.0}else{0.0}));
            assert_eq!(rt.current_variable("changed"),Some(if storage==16 {0.0}else{1.0}));
            rt.step(2,-1,100.0,3).unwrap();
            assert_eq!(rt.current_variable("inherited"),Some(if storage==16 {-3.0}else{2.0}));
        }
    }
    #[test]
    fn simple_function_signature_does_not_replace_saved_variable_history() {
        let functions=BTreeMap::from([("mp".into(),"mp=MarketPosition(0)*CurrentContracts;".into())]);
        let mut p=Program::with_functions("Vars: saved(0), old(0); saved=mp; old=saved[1];",&BTreeMap::new(),&functions).unwrap();
        p.set_function_storage_types(&BTreeMap::from([("mp".into(),16)])).unwrap();
        let bars=vec![sample(1261021,1000),sample(1261021,1100)];let mut rt=Runtime::new(&p,&bars);
        rt.step(0,0,0.0,0).unwrap();rt.step(1,1,100.0,2).unwrap();assert_eq!(rt.current_variable("old"),Some(0.0));
    }
    #[test]
    fn simple_host_expression_is_called_only_in_selected_branch_and_series_caller_keeps_history() {
        let functions: BTreeMap<String,String>=BTreeMap::from([("unit".into(),"unit=1/CurrentContracts;".into()),("wrapper".into(),"wrapper=unit;".into())]);
        // Separate proof of conditional execution: an unselected Simple call
        // must not divide by zero on a flat bar.
        let mut p=Program::with_functions("Vars: value(0); if CurrentContracts>0 then value=unit;",&BTreeMap::new(),
            &BTreeMap::from([("unit".into(),functions["unit"].clone())])).unwrap();
        p.set_function_storage_types(&BTreeMap::from([("unit".into(),16)])).unwrap();
        let bars=vec![sample(1261021,1000),sample(1261021,1100)];let mut rt=Runtime::new(&p,&bars);
        rt.step(0,0,0.0,0).unwrap();rt.step(1,1,100.0,2).unwrap();assert_eq!(rt.current_variable("value"),Some(0.5));
        let mut p=Program::with_functions("Vars: prior(0); prior=wrapper[1];",&BTreeMap::new(),&functions).unwrap();
        p.set_function_storage_types(&BTreeMap::from([("unit".into(),16),("wrapper".into(),32)])).unwrap();
        let mut rt=Runtime::new(&p,&bars);rt.step(0,1,100.0,2).unwrap();rt.step(1,-1,100.0,4).unwrap();
        assert_eq!(rt.current_variable("prior"),Some(0.5));
    }
    #[test]
    fn simple_signature_rejects_unverified_state_and_market_data_instead_of_silently_using_series() {
        for source in ["mp=Close;", "Vars: n(0); n=n+1; mp=n;", "if MarketPosition(0)=1 then mp=1;", "mp=mp+1;", "mp=Average(Close,2);"] {
            let functions=BTreeMap::from([("mp".into(),source.into())]);
            let mut p=Program::with_functions("Vars: result(0); result=mp;",&BTreeMap::new(),&functions).unwrap();
            assert!(p.set_function_storage_types(&BTreeMap::from([("mp".into(),16)])).is_err());
        }
        let functions=BTreeMap::from([("mp".into(),"mp=1;".into())]);
        let mut p=Program::with_functions("Vars: result(0); result=mp;",&BTreeMap::new(),&functions).unwrap();
        for types in [BTreeMap::from([("unknown".into(),16)]),BTreeMap::from([("mp".into(),7)]),BTreeMap::from([("mp".into(),16),("MP".into(),32)])] {
            assert!(p.set_function_storage_types(&types).is_err());
        }
    }
    #[test]
    fn unnamed_short_uses_native_short_label_and_preserves_explicit_labels() {
        let bars=vec![sample(1261021,1000)];
        let p=Program::compile("SellShort next bar at market; SellShort (\"SellShort\") next bar at market; SellShort (\"\") next bar at market;",&BTreeMap::new()).unwrap();
        let orders=Runtime::new(&p,&bars).step(0,0,0.0,0).unwrap();
        assert_eq!(orders.iter().map(|o|o.label.as_str()).collect::<Vec<_>>(),vec!["Short","SellShort",""]);
    }
    #[test]
    fn product_minmove_and_pricescale_come_from_the_same_host_specification() {
        let p=Program::compile("Vars: tick(0); tick = MinMove / PriceScale;",&BTreeMap::new()).unwrap();
        let bars=vec![sample(1261021,1000)];let mut rt=Runtime::new(&p,&bars);
        rt.execution=ExecutionContext{price_scale:Some(100.0),min_move:Some(25.0),..ExecutionContext::default()};
        rt.step(0,0,0.0,0).unwrap();assert_eq!(rt.current_variable("tick"),Some(0.25));
        let mut missing=Runtime::new(&p,&bars);assert!(missing.step(0,0,0.0,0).is_err());
    }
    #[test]
    fn mc_predeclared_values_and_conditions_preserve_initial_state_and_history() {
        let p=Program::compile("value3 = value3 + 1; condition81 = value3 > 1; if condition81 and value3[1] = 1 then Buy next bar at market;",&BTreeMap::new()).unwrap();
        let bars=vec![sample(1261021,1000),sample(1261021,1100)];let mut rt=Runtime::new(&p,&bars);
        assert!(rt.step(0,0,0.0,0).unwrap().is_empty());assert_eq!(rt.step(1,0,0.0,0).unwrap().len(),1);
        for source in ["unknown = 0;","value100 = 0;","condition100 = True;","value01 = 0;"] {
            assert!(Program::compile(source,&BTreeMap::new()).is_err(),"{source}");
        }
        let literal=Program::compile("Buy (\"value99\") next bar at market;",&BTreeMap::new()).unwrap();
        assert!(!literal.variables.contains_key("value99"));
    }
    #[test]
    fn native_medium_comparison_is_default_but_explicit_exact_stays_local() {
        let functions=BTreeMap::from([("native_cmp".into(),"native_cmp = 0 = 0.000000000001;".into())]);
        let p=Program::with_functions("Vars: inherited(0), exact(0), child(0); inherited = 0 = 0.000000000001; SetFPCompareAccuracy(FPCExact); exact = 0 = 0.000000000001; child = native_cmp;",&BTreeMap::new(),&functions).unwrap();
        let bars=vec![sample(1261021,1000)];let mut rt=Runtime::new(&p,&bars);
        rt.step(0,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("inherited"),Some(1.0));
        assert_eq!(rt.current_variable("exact"),Some(0.0));
        assert_eq!(rt.current_variable("child"),Some(1.0));
    }
    #[test]
    fn swing_zero_strength_and_empty_native_search_return_real_boundary_results() {
        let p=Program::compile("Vars: h(0), l(0), empty(0), zero_len(0); h=SwingHigh(1,High,0,1); l=SwingLow(1,Low,0,1); empty=SwingHigh(1,High,2,2); zero_len=SwingLow(1,Low,0,0);",&BTreeMap::new()).unwrap();
        let bars=numeric_bars(&[100.0]);let mut rt=Runtime::new(&p,&bars);rt.step(0,0,0.0,0).unwrap();
        assert_eq!(rt.current_variable("h"),Some(110.0));assert_eq!(rt.current_variable("l"),Some(90.0));
        assert_eq!(rt.current_variable("empty"),Some(-1.0));assert_eq!(rt.current_variable("zero_len"),Some(-1.0));
    }
}
