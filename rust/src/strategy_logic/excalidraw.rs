use super::{Edge, Flow};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const STEP_X: i32 = 640;
const WIDTH: i32 = 480;
const GAP: i32 = 120;

struct Card {
    id: usize,
    kind: &'static str,
    text: String,
    x: i32,
    y: i32,
    h: i32,
    reachable: bool,
}

fn wrap(text: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for c in text.chars() {
        let cells = if c.is_ascii() { 1 } else { 2 };
        if c == '\n' || width + cells > limit {
            result.push('\n');
            width = 0;
        }
        if c != '\n' {
            result.push(c);
            width += cells;
        }
    }
    result
}

fn card(id: usize, kind: &'static str, text: &str, x: i32, y: i32, reachable: bool) -> Card {
    let decision = matches!(kind, "condition" | "loop");
    let text = wrap(text, if decision { 32 } else { 44 });
    let lines = text.lines().count() as i32;
    let h = if decision {
        (lines * 24 + 26) * 2
    } else {
        (lines * 24 + 30).max(70)
    };
    Card {
        id,
        kind,
        text,
        x,
        y,
        h,
        reachable,
    }
}

fn columns(items: &[Flow]) -> i32 {
    items
        .iter()
        .map(|node| {
            node.branches
                .iter()
                .map(|branch| columns(&branch.items))
                .sum::<i32>()
        })
        .max()
        .unwrap_or(0)
        .max(1)
}

fn layout(items: &[Flow], left: i32, mut y: i32, cards: &mut BTreeMap<usize, Card>) -> i32 {
    let span = columns(items);
    for node in items {
        let item = card(
            node.id,
            node.kind,
            &format!("#{}  {}", node.id, node.text),
            left + (span - 1) * STEP_X / 2,
            y,
            node.reachable,
        );
        y += item.h + GAP;
        cards.insert(node.id, item);
        if !node.branches.is_empty() {
            let width: i32 = node
                .branches
                .iter()
                .map(|branch| columns(&branch.items))
                .sum();
            let mut lane = left + (span - width) * STEP_X / 2;
            let mut bottom = y;
            for branch in &node.branches {
                bottom = bottom.max(layout(&branch.items, lane, y, cards));
                lane += columns(&branch.items) * STEP_X;
            }
            y = bottom;
        }
    }
    y
}

fn element(id: &str, kind: &str, x: i32, y: i32, width: i32, height: i32, color: &str) -> Value {
    let seed = id.bytes().fold(17_u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    }) % 1_000_000
        + 1;
    json!({"id":id,"type":kind,"x":x,"y":y,"width":width,"height":height,"angle":0,
        "strokeColor":color,"backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,
        "strokeStyle":"solid","roughness":0,"opacity":100,"groupIds":[],"frameId":null,
        "roundness":null,"seed":seed,"version":1,"versionNonce":seed,"isDeleted":false,
        "boundElements":[],"updated":1,"link":null,"locked":false})
}

fn text(id: &str, label: &str, x: i32, y: i32, width: i32, color: &str, align: &str) -> Value {
    let mut item = element(
        id,
        "text",
        x,
        y,
        width,
        label.lines().count() as i32 * 24,
        color,
    );
    item.as_object_mut().unwrap().extend(json!({
        "text":label,"originalText":label,"fontSize":18,"fontFamily":5,
        "textAlign":align,"verticalAlign":"middle","containerId":null,"autoResize":false,"lineHeight":1.3
    }).as_object().unwrap().clone());
    item
}

pub(super) fn diagram(name: &str, flow: &[Flow], edges: &[Edge], end: usize) -> Value {
    let mut cards = BTreeMap::new();
    let center = (columns(flow) - 1) * STEP_X / 2;
    let start = card(0, "terminal", "開始評估", center, 118, true);
    let bottom = layout(flow, 0, start.y + start.h + GAP, &mut cards);
    cards.insert(0, start);
    cards.insert(
        end,
        card(end, "terminal", "結束本次評估", center, bottom, true),
    );
    let mut elements = Vec::new();
    let mut refs = BTreeMap::<usize, Vec<Value>>::new();
    for (index, edge) in edges.iter().enumerate() {
        let from = &cards[&edge.from];
        let to = &cards[&edge.to];
        let backwards = to.y <= from.y;
        let color = if edge.reachable { "#52647a" } else { "#8893a1" };
        let (x, y, mut points) = if backwards {
            let x = from.x - 7;
            let y = from.y + from.h / 2;
            let outer = from.x.min(to.x) - 58 - (index % 3) as i32 * 18;
            let target_y = to.y + to.h / 2 - y;
            (
                x,
                y,
                vec![
                    [0, 0],
                    [outer - x, 0],
                    [outer - x, target_y],
                    [to.x - 7 - x, target_y],
                ],
            )
        } else {
            let offset = if !edge.label.is_empty() && from.x == to.x {
                if edge.label == "否" || edge.label == "離開迴圈" {
                    WIDTH / 2 + 28
                } else {
                    WIDTH / 2
                }
            } else {
                WIDTH / 2
            };
            let x = from.x + offset;
            let y = from.y + from.h + 7;
            let dx = to.x + WIDTH / 2 - x;
            let dy = to.y - 7 - y;
            let elbow = (dy / 2).min(42);
            (x, y, vec![[0, 0], [0, elbow], [dx, elbow], [dx, dy]])
        };
        points.dedup();
        let id = format!("edge-{index}");
        let width =
            points.iter().map(|p| p[0]).max().unwrap() - points.iter().map(|p| p[0]).min().unwrap();
        let height =
            points.iter().map(|p| p[1]).max().unwrap() - points.iter().map(|p| p[1]).min().unwrap();
        let mut arrow = element(&id, "arrow", x, y, width, height, color);
        arrow["points"] = json!(points);
        arrow["startArrowhead"] = Value::Null;
        arrow["endArrowhead"] = json!("arrow");
        arrow["elbowed"] = json!(false);
        arrow["strokeStyle"] = json!(if edge.reachable { "solid" } else { "dashed" });
        for (key, node) in [("startBinding", from), ("endBinding", to)] {
            arrow[key] = json!({"elementId":format!("node-{}",node.id),"focus":0,"gap":7,
                "fixedPoint":null});
            let entries = refs.entry(node.id).or_default();
            if !entries.iter().any(|entry| entry["id"] == id) {
                entries.push(json!({"id":id,"type":"arrow"}));
            }
        }
        elements.push(arrow);
        if !edge.label.is_empty() {
            let label = if edge.reachable {
                edge.label.to_owned()
            } else {
                format!("{}（靜態不可達）", edge.label)
            };
            let left = backwards
                || to.x < from.x
                || (to.x == from.x && matches!(edge.label, "是" | "繼續迴圈"));
            let lx = if left { x - 183 } else { x + 8 };
            elements.push(text(
                &format!("edge-label-{index}"),
                &label,
                lx,
                y + 9,
                180,
                "#334155",
                if left { "right" } else { "left" },
            ));
        }
    }
    for node in cards.values() {
        let (kind, fill) = match node.kind {
            "condition" | "loop" => ("diamond", "#fff3bf"),
            "order" => ("rectangle", "#b2f2bb"),
            "terminal" => ("ellipse", "#c5f6fa"),
            _ => ("rectangle", "#d0ebff"),
        };
        let id = format!("node-{}", node.id);
        let label = format!("label-{}", node.id);
        let mut shape = element(&id, kind, node.x, node.y, WIDTH, node.h, "#334155");
        shape["backgroundColor"] = json!(if node.reachable { fill } else { "#f1f3f5" });
        shape["strokeStyle"] = json!(if node.reachable { "solid" } else { "dashed" });
        let mut bindings = refs.remove(&node.id).unwrap_or_default();
        bindings.push(json!({"id":label,"type":"text"}));
        shape["boundElements"] = json!(bindings);
        elements.push(shape);
        let label_width = if kind == "diamond" {
            WIDTH / 2
        } else {
            WIDTH - 32
        };
        let lines = node.text.lines().count() as i32;
        let mut caption = text(
            &label,
            &node.text,
            node.x + (WIDTH - label_width) / 2,
            node.y + (node.h - lines * 24) / 2,
            label_width,
            "#172b3c",
            "center",
        );
        caption["containerId"] = json!(id);
        elements.push(caption);
    }
    elements.push(text(
        "title",
        &wrap(&format!("{name}｜每棒策略流程"), 44),
        center,
        18,
        WIDTH,
        "#172b3c",
        "center",
    ));
    json!({"type":"excalidraw","version":2,"source":"https://excalidraw.com","elements":elements,
        "appState":{"viewBackgroundColor":"#ffffff","gridSize":null},"files":{}})
}
