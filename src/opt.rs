// Assembly-level cleanup of one function: removes unreachable code. Code that a
// constant condition makes dead (`if (0) f();`) must not even be emitted, since
// programs rely on it not referencing symbols that do not exist.
use std::collections::HashMap;

fn is_label(l: &str) -> bool {
    !l.starts_with(' ') && l.ends_with(':')
}

fn is_insn(l: &str) -> bool {
    l.starts_with("  ") && !l[2..].starts_with('.')
}

/// Labels the code generator invented and only jumps to from the same function.
/// User labels (`.L.u.`) can be referenced from data, so they always stay.
fn removable(label: &str) -> bool {
    label.starts_with(".L.") && !label.starts_with(".L.u.")
}

/// Counts the references to `.L` labels in non-label lines.
fn count_refs<'a>(lines: &[&'a str]) -> HashMap<&'a str, usize> {
    let mut refs = HashMap::new();
    for l in lines.iter().filter(|l| !is_label(l)) {
        let mut rest = *l;
        while let Some(i) = rest.find(".L") {
            let tail = &rest[i..];
            let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_')).unwrap_or(tail.len());
            *refs.entry(&tail[..end]).or_insert(0) += 1;
            rest = &tail[end..];
        }
    }
    refs
}

pub fn remove_dead_code(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    loop {
        let refs = count_refs(&lines);
        let mut out: Vec<&str> = Vec::with_capacity(lines.len());
        let mut dead = false;
        for (i, l) in lines.iter().enumerate() {
            if is_label(l) {
                let name = &l[..l.len() - 1];
                if removable(name) && !refs.contains_key(name) {
                    continue;
                }
                dead = false;
            } else if is_insn(l) {
                if dead {
                    continue;
                }
                if let Some(target) = l.strip_prefix("  jmp ") {
                    // A jump to the label right after it does nothing.
                    if lines[i + 1..].iter().take_while(|n| is_label(n)).any(|n| n[..n.len() - 1] == *target) {
                        continue;
                    }
                    dead = true;
                } else if *l == "  ret" {
                    dead = true;
                }
            }
            out.push(l);
        }
        if out.len() == lines.len() {
            break;
        }
        lines = out;
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}
