use serde_json::{json, Value};
use std::{fmt::Write, fs, path::Path};

enum Edit<'a> {
    Same,
    Remove(&'a str),
    Add(&'a str),
}

fn lcs_table<T: Eq>(left: &[T], right: &[T]) -> Vec<u32> {
    let width = right.len() + 1;
    let mut table = vec![0; (left.len() + 1) * width];
    for i in (0..left.len()).rev() {
        for j in (0..right.len()).rev() {
            table[i * width + j] = if left[i] == right[j] {
                1 + table[(i + 1) * width + j + 1]
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    table
}

pub fn compare(before: &str, after: &str) -> Result<Value, String> {
    if before.len() > 100_000 || after.len() > 100_000 {
        return Err("比較的每份 PowerLanguage 原始碼上限為 100,000 位元組".into());
    }
    let before = before.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let after = after.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let old: Vec<_> = before.split_terminator('\n').collect();
    let new: Vec<_> = after.split_terminator('\n').collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let a = &old[prefix..old.len() - suffix];
    let b = &new[prefix..new.len() - suffix];
    let width = b.len() + 1;
    let cells = (a.len() + 1).saturating_mul(width);
    let coarse = !a.is_empty() && !b.is_empty() && cells > 4_000_000;
    let mut edits = vec![];
    edits.extend((0..prefix).map(|_| Edit::Same));
    if a.is_empty() || b.is_empty() || coarse {
        edits.extend(a.iter().map(|line| Edit::Remove(line)));
        edits.extend(b.iter().map(|line| Edit::Add(line)));
    } else {
        let lcs = lcs_table(a, b);
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                edits.push(Edit::Same);
                i += 1;
                j += 1;
            } else if i < a.len()
                && (j == b.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                edits.push(Edit::Remove(a[i]));
                i += 1;
            } else {
                edits.push(Edit::Add(b[j]));
                j += 1;
            }
        }
    }
    edits.extend((0..suffix).map(|_| Edit::Same));
    let (mut old_line, mut new_line, mut added, mut removed) = (1, 1, 0, 0);
    let mut changes = vec![];
    for edit in edits {
        match edit {
            Edit::Same => {
                old_line += 1;
                new_line += 1;
            }
            Edit::Remove(text) => {
                changes
                    .push(json!({"op":"remove","old_line":old_line,"new_line":null,"text":text}));
                old_line += 1;
                removed += 1;
            }
            Edit::Add(text) => {
                changes.push(json!({"op":"add","old_line":null,"new_line":new_line,"text":text}));
                new_line += 1;
                added += 1;
            }
        }
    }
    let final_newline_changed = before.ends_with('\n') != after.ends_with('\n');
    Ok(
        json!({"identical":changes.is_empty() && !final_newline_changed,"added":added,
        "removed":removed,"changes":changes,"coarse":coarse,"final_newline_changed":final_newline_changed}),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn highlight_pair(left: &str, right: &str) -> (String, String) {
    let a: Vec<_> = left.chars().collect();
    let b: Vec<_> = right.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let left_core = &a[prefix..a.len() - suffix];
    let right_core = &b[prefix..b.len() - suffix];
    let width = right_core.len() + 1;
    let mut left_changes = vec![true; left_core.len()];
    let mut right_changes = vec![true; right_core.len()];
    if !left_core.is_empty()
        && !right_core.is_empty()
        && (left_core.len() + 1).saturating_mul(width) <= 16_000
    {
        let table = lcs_table(left_core, right_core);
        let (mut i, mut j) = (0, 0);
        while i < left_core.len() && j < right_core.len() {
            if left_core[i] == right_core[j] {
                left_changes[i] = false;
                right_changes[j] = false;
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let decorate = |chars: &[char], changes: &[bool]| {
        let safe = |chars: &[char]| escape(&chars.iter().collect::<String>());
        let mut rendered = safe(&chars[..prefix]);
        let mut start = 0;
        while start < changes.len() {
            let changed = changes[start];
            let end = start
                + changes[start..]
                    .iter()
                    .take_while(|&&item| item == changed)
                    .count();
            if changed {
                rendered.push_str("<mark>");
            }
            rendered.push_str(&safe(&chars[prefix + start..prefix + end]));
            if changed {
                rendered.push_str("</mark>");
            }
            start = end;
        }
        rendered.push_str(&safe(&chars[chars.len() - suffix..]));
        rendered
    };
    (decorate(&a, &left_changes), decorate(&b, &right_changes))
}

fn report_html(before: &str, after: &str, diff: &Value) -> String {
    let before = before.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let after = after.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let old: Vec<_> = before.split_terminator('\n').collect();
    let new: Vec<_> = after.split_terminator('\n').collect();
    let edits = diff["changes"].as_array().expect("compare returns changes");
    let mut html = String::from(
        r##"<!doctype html><html lang="zh-Hant"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>PowerLanguage 修改前後比對</title>
<style>
:root{color-scheme:dark;--ink:#e9edf1;--muted:#a2adbc;--divider:#364254;--accent:#93dcd1}
*{box-sizing:border-box}body{margin:0;background:#11161d;color:var(--ink);font:14px system-ui,sans-serif;padding-right:27px}
header{position:sticky;top:0;z-index:2;background:#1a2330;border-bottom:1px solid var(--divider);padding:12px 16px;display:flex;align-items:center;gap:14px;justify-content:space-between;flex-wrap:wrap}
h1{font-size:17px;margin:0;font-weight:650}.summary{margin-left:9px;font-size:13px;white-space:nowrap}
.total-add{color:#98e5bd}.total-remove{color:#ffb5be}.toolbar{display:flex;gap:6px;align-items:center;flex-wrap:wrap}
button{font:inherit;color:inherit;background:#232e3b;border:1px solid #4c5d72;border-radius:5px;padding:4px 9px;cursor:pointer}
button:hover{background:#304052}button:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
#position{min-width:5ch;color:var(--muted);font-variant-numeric:tabular-nums;text-align:center}
table{width:100%;table-layout:fixed;border-collapse:separate;border-spacing:0}th{text-align:left;background:#212d3a;padding:8px 12px;color:var(--muted)}
td{vertical-align:top}.code{padding:2px 9px;white-space:pre-wrap;overflow-wrap:anywhere;font:14px/1.45 Consolas,"Microsoft JhengHei",monospace;tab-size:4}
.num{width:5em;text-align:right;padding:2px 8px;color:#a6b4c3;user-select:none;font:14px/1.45 Consolas,monospace;background:#141c26}
.divider{border-left:2px solid var(--divider)}.removed{background:#35232b}.added{background:#19352c}
.num.removed{color:#ffb5be}.num.added{color:#98e5bd}
.blank{background:repeating-linear-gradient(135deg,#141c26,#141c26 5px,#1a2330 5px,#1a2330 6px)}
mark{color:inherit;border-radius:2px}.removed mark{background:#8b3949;color:#fff}.added mark{background:#216952;color:#fff}
tr.change{scroll-margin-top:110px}tr.active td{box-shadow:inset 0 2px #f4d389,inset 0 -1px #f4d389}
.notice{padding:0 16px;color:#f3d8a0}
.fold-row td{text-align:center;padding:4px 0;background:#1c2835;border-block:1px solid #364659}
.fold-row button{background:transparent;border:0;color:var(--accent)}[hidden]{display:none!important}
#overview{position:fixed;right:5px;top:70px;bottom:12px;width:16px;border-left:1px solid var(--divider)}
#overview button{position:absolute;left:2px;width:11px;min-height:5px;padding:0;border:0;border-radius:1px;background:#d18b8e}
#overview button[data-kind=added]{background:#6fbc96}#overview button[data-kind=mixed]{background:#e0c480}
</style><body><header><h1>PowerLanguage 修改前後比對</h1>"##,
    );
    write!(
        html,
        "<span class=\"summary\"><span class=\"total-add\">+{} 行</span> · <span class=\"total-remove\">−{} 行</span></span>",
        diff["added"], diff["removed"]
    )
    .unwrap();
    if !edits.is_empty() {
        html.push_str("<nav class=\"toolbar\" aria-label=\"比對工具\"><button id=\"prev\" title=\"Shift+F7\">上一處</button><button id=\"next\" title=\"F7\">下一處</button><span id=\"position\" aria-live=\"polite\"></span><button id=\"fold-all\" hidden>展開未變更區段</button></nav>");
    }
    html.push_str("</header>");
    if diff["final_newline_changed"] == true {
        html.push_str("<p class=\"notice\">檔尾換行有變更。</p>");
    }
    if diff["coarse"] == true {
        html.push_str("<p class=\"notice\">大型變更區塊逐行呈現，行內比對已略過。</p>");
    }
    html.push_str("<table aria-label=\"原版與改版左右對照\"><colgroup><col class=\"num\"><col><col class=\"num\"><col></colgroup><thead><tr><th colspan=\"2\">修改前</th><th colspan=\"2\" class=\"divider\">修改後</th></tr></thead><tbody>");
    let cell = |html: &mut String,
                index: usize,
                code: Option<&str>,
                style: &str,
                divider: &str,
                marker: &str| {
        if let Some(code) = code {
            write!(html,"<td class=\"num {style} {divider}\">{marker}{index}</td><td class=\"code {style}\">{code}</td>").unwrap();
        } else {
            write!(html, "<td class=\"blank {divider}\" colspan=\"2\"></td>").unwrap();
        }
    };
    let (mut i, mut j, mut k, mut block) = (0, 0, 0, 0);
    while i < old.len() || j < new.len() {
        let (start_i, start_j) = (i, j);
        while let Some(edit) = edits.get(k) {
            if edit["old_line"].as_u64() == Some(i as u64 + 1) {
                i += 1;
            } else if edit["new_line"].as_u64() == Some(j as u64 + 1) {
                j += 1;
            } else {
                break;
            }
            k += 1;
        }
        if (i, j) == (start_i, start_j) {
            html.push_str("<tr>");
            let left = old.get(i).map(|text| escape(text));
            let right = new.get(j).map(|text| escape(text));
            cell(&mut html, i + 1, left.as_deref(), "", "", "");
            cell(&mut html, j + 1, right.as_deref(), "", "divider", "");
            i += 1;
            j += 1;
            html.push_str("</tr>");
            continue;
        }
        block += 1;
        for offset in 0..(i - start_i).max(j - start_j) {
            let x = start_i + offset;
            let y = start_j + offset;
            let left = (x < i).then(|| old[x]);
            let right = (y < j).then(|| new[y]);
            let (left, right) = match (left, right) {
                (Some(a), Some(b)) if diff["coarse"] != true => {
                    let (a, b) = highlight_pair(a, b);
                    (Some(a), Some(b))
                }
                _ => (left.map(escape), right.map(escape)),
            };
            write!(html, "<tr class=\"change\" data-block=\"{block}\">").unwrap();
            cell(&mut html, x + 1, left.as_deref(), "removed", "", "−");
            cell(&mut html, y + 1, right.as_deref(), "added", "divider", "+");
            html.push_str("</tr>");
        }
    }
    html.push_str(r#"</tbody></table><nav id="overview" aria-label="變更位置總覽" hidden></nav><script>
const rows = Array.from(document.querySelector('tbody').rows);
const blocks = [];
const folds = [];
const overview = document.getElementById('overview');
for (let i = 0; i < rows.length;) {
  const start = i;
  const key = rows[i].dataset.block;
  while (i < rows.length && rows[i].dataset.block === key) i++;
  if (key) {
    blocks.push({start, rows: rows.slice(start, i)});
  } else if (i - start >= 10 && document.getElementById('next')) {
    const hiddenRows = rows.slice(start + (start > 0 ? 3 : 0), i - (i < rows.length ? 3 : 0));
    const separator = document.createElement('tr');
    separator.className = 'fold-row';
    const cell = separator.insertCell();
    cell.colSpan = 4;
    const button = document.createElement('button');
    cell.append(button);
    hiddenRows[0].before(separator);
    const toggle = (expanded) => {
      hiddenRows.forEach(row => row.hidden = !expanded);
      button.setAttribute('aria-expanded', String(expanded));
      button.textContent = (expanded ? '收合 ' : '展開 ') + hiddenRows.length + ' 行未變更內容';
    };
    button.addEventListener('click', () => toggle(button.getAttribute('aria-expanded') !== 'true'));
    toggle(false);
    folds.push(toggle);
  }
}
let selected = -1;
function goTo(index) {
  if (!blocks.length) return;
  blocks[selected]?.rows.forEach(row => row.classList.remove('active'));
  selected = (index + blocks.length) % blocks.length;
  const current = blocks[selected].rows;
  current.forEach(row => row.classList.add('active'));
  current[0].scrollIntoView({block:'center'});
  document.getElementById('position').textContent = (selected + 1) + ' / ' + blocks.length;
}
document.getElementById('prev')?.addEventListener('click', () => goTo(selected < 0 ? blocks.length - 1 : selected - 1));
document.getElementById('next')?.addEventListener('click', () => goTo(selected + 1));
if (blocks.length) {
  overview.hidden = false;
  blocks.forEach((block, index) => {
    const button = document.createElement('button');
    const removed = block.rows.some(row => row.querySelector('.removed'));
    const added = block.rows.some(row => row.querySelector('.added'));
    button.dataset.kind = removed && added ? 'mixed' : added ? 'added' : 'removed';
    button.style.top = (block.start * 96 / rows.length) + '%';
    button.title = '跳至第 ' + (index + 1) + ' 處變更';
    button.setAttribute('aria-label', button.title);
    button.addEventListener('click', () => goTo(index));
    overview.append(button);
  });
  document.getElementById('position').textContent = '– / ' + blocks.length;
  document.addEventListener('keydown', event => {
    if (event.key !== 'F7') return;
    event.preventDefault();
    goTo(event.shiftKey && selected < 0 ? blocks.length - 1 : selected + (event.shiftKey ? -1 : 1));
  });
}
const toggleAll = document.getElementById('fold-all');
if (folds.length && toggleAll) {
  let expanded = false;
  toggleAll.hidden = false;
  toggleAll.addEventListener('click', () => {
    expanded = !expanded;
    folds.forEach(toggle => toggle(expanded));
    toggleAll.textContent = expanded ? '收合未變更區段' : '展開未變更區段';
  });
}
</script></body></html>"#);
    html
}

pub fn compare_and_export(root: &Path, before: &str, after: &str) -> Result<Value, String> {
    let mut result = compare(before, after)?;
    let parent = root.join("outputs/powerlanguage-diff");
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let dir = parent.join(crate::strategy_library::fresh_id());
    fs::create_dir(&dir).map_err(|error| error.to_string())?;
    let path = dir.join("comparison.html");
    fs::write(&path, report_html(before, after, &result)).map_err(|error| error.to_string())?;
    result["report_path"] = json!(path);
    Ok(result)
}
