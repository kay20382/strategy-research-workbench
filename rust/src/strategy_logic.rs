//! Static checks and control flow derived from the same PL program used by backtests.
use crate::{
    powerlanguage::{Action, Expr, Kind, Node, OrderSpec, Stmt},
    strategy_library::{self, Document},
    strategy_research,
};
mod excalidraw;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Serialize)]
struct Flow {
    id: usize,
    kind: &'static str,
    text: String,
    reachable: bool,
    branches: Vec<Branch>,
}

#[derive(Serialize)]
struct Branch {
    label: &'static str,
    reachable: bool,
    items: Vec<Flow>,
}

#[derive(Serialize)]
struct Finding {
    code: &'static str,
    level: &'static str,
    nodes: Vec<usize>,
    message: String,
}

#[derive(Default)]
struct Review {
    count: usize,
    findings: Vec<Finding>,
    orders: Vec<(usize, Action, Kind)>,
    labels: BTreeMap<String, Vec<usize>>,
}

fn expression(expr: &Expr) -> String {
    match &expr.node {
        Node::Number(n) => n.to_string(),
        Node::Text(text) => format!("{text:?}"),
        Node::Name(name) => name.clone(),
        Node::Unary(op, expr) => format!("({op} {})", expression(expr)),
        Node::Binary(op, left, right) => format!(
            "({} {} {})",
            expression(left),
            op.replace('_', " "),
            expression(right)
        ),
        Node::Call(name, args) => format!(
            "{name}({})",
            args.iter().map(expression).collect::<Vec<_>>().join(", ")
        ),
        Node::History(base, index) => format!("{}[{}]", expression(base), expression(index)),
        Node::ArrayIndex(name, index) => format!("{name}[{}]", expression(index)),
    }
}

fn number(expr: &Expr) -> Option<f64> {
    match &expr.node {
        Node::Number(n) => Some(*n),
        Node::Unary(op, expr) if op == "-" => Some(-number(expr)?),
        _ => None,
    }
}

fn literal_condition(expr: &Expr) -> Option<bool> {
    match &expr.node {
        Node::Unary(op, expr) if op == "not" => Some(!literal_condition(expr)?),
        Node::Binary(op, left, right) if op == "and" || op == "or" => {
            if op == "and" {
                if let (Some((a, x)), Some((b, y))) =
                    (integer_equality(left), integer_equality(right))
                {
                    if a == b && (x - y).abs() >= 1.0 {
                        return Some(false);
                    }
                }
            }
            let (left, right) = (literal_condition(left)?, literal_condition(right)?);
            Some(if op == "and" {
                left && right
            } else {
                left || right
            })
        }
        _ => Some(number(expr)? != 0.0),
    }
}

fn integer_equality(expr: &Expr) -> Option<(&str, f64)> {
    let Node::Binary(op, left, right) = &expr.node else {
        return None;
    };
    if op != "=" {
        return None;
    }
    let (name, value) = match (&left.node, &right.node) {
        (Node::Name(name), _) => (name, number(right)?),
        (_, Node::Name(name)) => (name, number(left)?),
        _ => return None,
    };
    // Restrict comparisons to small integers and keep a wide gap relative to comparison tolerance.
    (value.fract() == 0.0 && value.abs() <= 10_000.0).then_some((name, value))
}

fn order_text(order: &OrderSpec) -> String {
    let size = order
        .quantity
        .as_ref()
        .map(|expr| {
            format!(
                "{} 口{}",
                expression(expr),
                if order.total { "（合計）" } else { "" }
            )
        })
        .unwrap_or_else(|| match order.action {
            Action::Buy | Action::SellShort => "依策略預設口數".into(),
            Action::Sell | Action::BuyToCover => "平符合方向的全部部位".into(),
        });
    let price = order.price.as_ref().map(expression).unwrap_or_default();
    let timing = match order.kind {
        Kind::Market => "次棒市價".into(),
        Kind::Limit => format!("次棒限價 {price}"),
        Kind::Stop => format!("次棒停損價 {price}"),
        Kind::Close => "本棒收盤".into(),
    };
    format!(
        "{}（{}）｜{size}｜{timing}",
        order.action.name(),
        order.label
    )
}

impl Review {
    fn note(
        &mut self,
        code: &'static str,
        level: &'static str,
        nodes: Vec<usize>,
        message: impl Into<String>,
    ) {
        self.findings.push(Finding {
            code,
            level,
            nodes,
            message: message.into(),
        });
    }

    fn block(&mut self, statements: &[Stmt], reachable: bool) -> Result<Vec<Flow>, String> {
        let mut items = Vec::new();
        let mut buy = None;
        let mut sell_short = None;
        for statement in statements {
            self.count += 1;
            let id = self.count;
            if id > 2000 {
                return Err("流程圖超過 2000 個節點，請縮小檢查的策略範圍".into());
            }
            let mut branches = Vec::new();
            let (kind, text) = match statement {
                Stmt::If(condition, yes, no) => {
                    let constant = literal_condition(condition);
                    if reachable {
                        if let Some(constant) = constant {
                            self.note(
                                "constant_condition",
                                "warning",
                                vec![id],
                                if constant {
                                    "此條件固定成立，否分支無法執行。"
                                } else {
                                    "此條件固定為假，是分支無法執行。"
                                },
                            );
                        }
                    }
                    for (label, possible, body) in [
                        ("是", constant != Some(false), yes),
                        ("否", constant != Some(true), no),
                    ] {
                        let possible = reachable && possible;
                        branches.push(Branch {
                            label,
                            reachable: possible,
                            items: self.block(body, possible)?,
                        });
                    }
                    ("condition", expression(condition))
                }
                Stmt::For(name, from, to, descending, body) => {
                    let empty = number(from).zip(number(to)).is_some_and(|(from, to)| {
                        // Integer bounds only. Other values are left to runtime semantics.
                        from.fract() == 0.0
                            && to.fract() == 0.0
                            && if *descending { from < to } else { from > to }
                    });
                    if reachable && empty {
                        self.note(
                            "empty_loop",
                            "warning",
                            vec![id],
                            "固定起訖值與迴圈方向不符，迴圈本文不會執行。",
                        );
                    }
                    let possible = reachable && !empty;
                    branches.push(Branch {
                        label: "繼續迴圈",
                        reachable: possible,
                        items: self.block(body, possible)?,
                    });
                    branches.push(Branch {
                        label: "離開迴圈",
                        reachable,
                        items: vec![],
                    });
                    (
                        "loop",
                        format!(
                            "{name} = {} {} {}",
                            expression(from),
                            if *descending { "downto" } else { "to" },
                            expression(to)
                        ),
                    )
                }
                Stmt::Order(order) => {
                    if reachable {
                        self.orders.push((id, order.action, order.kind));
                        match order.action {
                            Action::Buy => {
                                buy.get_or_insert(id);
                            }
                            Action::SellShort => {
                                sell_short.get_or_insert(id);
                            }
                            _ => {}
                        }
                        let label = order.label.to_lowercase();
                        if !label.is_empty()
                            && !["buy", "sell", "short", "sellshort", "buytocover"]
                                .contains(&label.as_str())
                        {
                            self.labels.entry(label).or_default().push(id);
                        }
                        if order
                            .quantity
                            .as_ref()
                            .and_then(number)
                            .is_some_and(|n| n < 1.0 || n.fract() != 0.0 || n > 1_000_000.0)
                        {
                            self.note(
                                "invalid_order_quantity",
                                "error",
                                vec![id],
                                "固定委託口數超出相容引擎接受的 1 至 1,000,000 正整數範圍。",
                            );
                        }
                    }
                    ("order", order_text(order))
                }
                Stmt::Assign(name, value) => {
                    ("assignment", format!("{name} = {}", expression(value)))
                }
                Stmt::ArrayAssign(name, index, value) => (
                    "assignment",
                    format!("{name}[{}] = {}", expression(index), expression(value)),
                ),
                Stmt::SetExactComparison => ("setting", "SetFPCompareAccuracy(FPCExact)".into()),
                Stmt::SetMediumComparison => {
                    ("setting", "SetFPCompareAccuracy(FPCMedAccuracy)".into())
                }
            };
            items.push(Flow {
                id,
                kind,
                text,
                reachable,
                branches,
            });
        }
        if let (Some(buy), Some(short)) = (buy, sell_short) {
            self.note("opposing_entries", "warning", vec![buy, short], "同一執行區塊包含買進與放空委託，兩者可能在同一次策略評估發出，請核對委託優先順序。");
        }
        Ok(items)
    }

    fn summarize(&mut self) {
        for (label, nodes) in std::mem::take(&mut self.labels) {
            if nodes.len() > 1 {
                self.note(
                    "reused_order_label",
                    "warning",
                    nodes,
                    format!(
                        "多條可到達的委託敘述使用同一標籤「{label}」，請核對原廠編譯與成交對帳。"
                    ),
                );
            }
        }
        let nodes_for = |action| {
            self.orders
                .iter()
                .filter_map(|(id, a, _)| (*a == action).then_some(*id))
                .collect::<Vec<_>>()
        };
        let long = nodes_for(Action::Buy);
        let short = nodes_for(Action::SellShort);
        let sell = nodes_for(Action::Sell);
        let cover = nodes_for(Action::BuyToCover);
        let close = self
            .orders
            .iter()
            .filter_map(|(id, _, kind)| (*kind == Kind::Close).then_some(*id))
            .collect::<Vec<_>>();
        if long.is_empty() && short.is_empty() {
            self.note(
                "no_entry",
                "warning",
                vec![],
                "策略主流程找不到可到達的進場委託。",
            );
        }
        if !long.is_empty() && sell.is_empty() {
            self.note(
                "missing_long_exit",
                "warning",
                long,
                "有買進委託，未找到對應的 Sell 平倉。若設計以反手出場，請用成交明細核對。",
            );
        }
        if !short.is_empty() && cover.is_empty() {
            self.note(
                "missing_short_exit",
                "warning",
                short,
                "有放空委託，未找到對應的 BuyToCover 平倉。若設計以反手出場，請用成交明細核對。",
            );
        }
        if !close.is_empty() {
            self.note(
                "bar_close_order",
                "warning",
                close,
                "使用本棒收盤委託；請以實際成交棒驗證每筆平倉 exit_bar > entry_bar。",
            );
        }
    }
}

#[derive(Serialize)]
struct Edge {
    from: usize,
    to: usize,
    label: &'static str,
    reachable: bool,
}

fn connect(items: &[Flow], next: usize, edges: &mut Vec<Edge>) -> usize {
    let mut next = next;
    for item in items.iter().rev() {
        if item.branches.is_empty() {
            edges.push(Edge {
                from: item.id,
                to: next,
                label: "",
                reachable: item.reachable,
            });
        }
        for branch in &item.branches {
            let tail = if item.kind == "loop" && branch.label == "繼續迴圈" {
                item.id
            } else {
                next
            };
            let entry = connect(&branch.items, tail, edges);
            edges.push(Edge {
                from: item.id,
                to: entry,
                label: branch.label,
                reachable: branch.reachable,
            });
        }
        next = item.id;
    }
    next
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render(items: &[Flow], html: &mut String) {
    for item in items {
        let class = if item.reachable { "" } else { " unreachable" };
        html.push_str(&format!(
            "<article id=\"n{}\" class=\"node {}{}\"><small>#{}</small><span>{}</span></article>",
            item.id,
            item.kind,
            class,
            item.id,
            escape(&item.text)
        ));
        if !item.branches.is_empty() {
            html.push_str("<div class=\"branches\">");
            for branch in &item.branches {
                let state = if branch.reachable {
                    ""
                } else {
                    "（靜態不可達）"
                };
                html.push_str(&format!(
                    "<section class=\"lane\"><h3>{}{state}</h3>",
                    branch.label
                ));
                render(&branch.items, html);
                if branch.label == "繼續迴圈" {
                    html.push_str(&format!("<a href=\"#n{}\">↶ 返回迴圈判斷</a>", item.id));
                } else if branch.items.is_empty() {
                    html.push_str("<p>繼續下一個步驟</p>");
                }
                html.push_str("</section>");
            }
            html.push_str("</div><div class=\"join\">↓</div>");
        }
    }
}

fn html_report(name: &str, items: &[Flow], findings: &[Finding], source_sha256: &str) -> String {
    let mut html = String::from(
        r#"<!doctype html><html lang="zh-Hant"><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>策略條件流程圖</title>
<style>
body{margin:0;background:#101821;color:#ebf1f6;font:16px/1.6 system-ui,sans-serif}main{max-width:1080px;margin:auto;padding:28px}
h1,h2,h3,p{margin:0 0 12px}small{color:#b5c5d4;margin-right:.8rem}a{color:#8be3d5}code{overflow-wrap:anywhere}
.audit,.node,.lane{border:1px solid #41586c;border-radius:8px;padding:12px 16px;background:#192633;margin:12px 0}
.node{position:relative;overflow-wrap:anywhere;scroll-margin-top:12px}.node:target{outline:2px solid #ffc77d}
.node+.node:before{content:"↓";position:absolute;top:-18px;left:49%;color:#b5c5d4}.order{border-left:5px solid #70d1be}
.condition,.loop{border-left:5px solid #ffc77d}.branches{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}
.lane{background:#13202b;border-top:3px solid #63869d}.unreachable{border-style:dashed;color:#a8b5c2}
.terminal,.join{text-align:center;color:#b9ded9}.error{color:#ffb3ab}.warning{color:#ffd399}
@media(max-width:620px){main{padding:12px}.branches{grid-template-columns:1fr}}
</style><main>"#,
    );
    html.push_str(&format!(
        "<h1>{}｜策略條件流程圖</h1><p>原始碼 SHA-256 <code>{source_sha256}</code></p>",
        escape(name)
    ));
    html.push_str("<p><a href=\"flow.excalidraw\" download>下載可編輯的 Excalidraw 流程圖</a></p>");
    html.push_str("<section class=\"audit\"><h2>策略邏輯檢查</h2><p>依相容引擎解析策略主流程。函式呼叫保留在運算式中；成交與跨棒規則須另以逐筆回測驗收。</p>");
    if findings.is_empty() {
        html.push_str("<p>已執行的靜態規則未發現問題。</p>");
    } else {
        html.push_str("<ul>");
        for finding in findings {
            let label = if finding.level == "error" {
                "問題"
            } else {
                "提醒"
            };
            html.push_str(&format!(
                "<li><span class=\"{}\">{label}</span> {}",
                finding.level,
                escape(&finding.message)
            ));
            for id in &finding.nodes {
                html.push_str(&format!(" <a href=\"#n{id}\">#{id}</a>"));
            }
            html.push_str("</li>");
        }
        html.push_str("</ul>");
    }
    html.push_str("</section><h2>每棒策略執行順序</h2><div class=\"terminal\">開始評估 ↓</div>");
    render(items, &mut html);
    html.push_str("<div class=\"terminal\">↓ 結束本次評估</div></main></html>");
    html
}

pub fn analyze(root: &Path, document: &Document, export_html: bool) -> Result<Value, String> {
    if document.kind != "strategy" {
        return Err("請選擇策略訊號以檢查條件流程".into());
    }
    let program = strategy_research::compile_document(root, document)?;
    let mut review = Review::default();
    let flow = review.block(&program.body, true)?;
    review.summarize();
    let end = review.count + 1;
    let mut edges = Vec::new();
    let first = connect(&flow, end, &mut edges);
    edges.push(Edge {
        from: 0,
        to: first,
        label: "",
        reachable: true,
    });
    let hash = format!("{:x}", Sha256::digest(document.source.as_bytes()));
    let (report_path, excalidraw_path) = if export_html {
        let parent = root.join("outputs/strategy-logic");
        fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let dir = parent.join(strategy_library::fresh_id());
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let file = dir.join("flow.html");
        fs::write(
            &file,
            html_report(&document.name, &flow, &review.findings, &hash),
        )
        .map_err(|e| e.to_string())?;
        let drawing = dir.join("flow.excalidraw");
        let editable = excalidraw::diagram(&document.name, &flow, &edges, end);
        fs::write(&drawing, serde_json::to_vec(&editable).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        (Some(file), Some(drawing))
    } else {
        (None, None)
    };
    Ok(json!({
        "schema":"strategy-logic-v1", "strategy":document.name, "source_sha256":hash,
        "finding_count":review.findings.len(),"findings":review.findings, "statement_count":review.count,
        "start":0,"end":end,"flow":flow,"edges":edges,"report_path":report_path,"excalidraw_path":excalidraw_path,
        "scope":"相容引擎的策略主流程靜態分析。實際可達性及成交須以行情與逐筆結果驗證。"
    }))
}
