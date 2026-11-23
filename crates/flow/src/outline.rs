//! A plan outline: a tree of tasks with their status, kept for the whole run
//! by *Plan outline* blocks and shown as indented text. Pure text handling:
//! tasks are matched by their text.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Pending,
    Working,
    Done,
    Failed,
    /// Broken into smaller steps.
    Split,
}

impl TaskStatus {
    fn mark(self) -> &'static str {
        match self {
            TaskStatus::Pending => "○",
            TaskStatus::Working => "▸",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
            TaskStatus::Split => "▾",
        }
    }

    fn open(self) -> bool {
        matches!(self, TaskStatus::Pending | TaskStatus::Working)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Task {
    text: String,
    status: TaskStatus,
    children: Vec<Task>,
}

/// Longest task line shown.
const LINE_CHARS: usize = 160;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    roots: Vec<Task>,
}

/// Task text for matching: without a "Task:" prefix, markdown stars or
/// extra spaces, in lower case.
fn norm(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("Task:")
        .or_else(|| s.strip_prefix("task:"))
        .unwrap_or(s);
    s.replace("**", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The text shown for a task: its first line, cut short.
fn line(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("Task:").unwrap_or(s).trim();
    let first = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let more = s.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let n = first.chars().count();
    if n > LINE_CHARS {
        first.chars().take(LINE_CHARS).collect::<String>() + "…"
    } else if more {
        format!("{first} …")
    } else {
        first.to_string()
    }
}

impl Outline {
    /// Starts again with one task.
    pub fn start(&mut self, goal: &str) {
        self.roots = vec![Task {
            text: goal.to_string(),
            ..Task::default()
        }];
    }

    /// Paths of every task, breadth-first (the order tasks are worked on).
    fn order(&self) -> Vec<Vec<usize>> {
        let mut out = Vec::new();
        let mut level: Vec<Vec<usize>> = (0..self.roots.len()).map(|i| vec![i]).collect();
        while !level.is_empty() {
            let mut next = Vec::new();
            for p in level {
                for i in 0..self.get(&p).children.len() {
                    let mut c = p.clone();
                    c.push(i);
                    next.push(c);
                }
                out.push(p);
            }
            level = next;
        }
        out
    }

    fn get(&self, path: &[usize]) -> &Task {
        let mut t = &self.roots[path[0]];
        for &i in &path[1..] {
            t = &t.children[i];
        }
        t
    }

    fn get_mut(&mut self, path: &[usize]) -> &mut Task {
        let mut t = &mut self.roots[path[0]];
        for &i in &path[1..] {
            t = &mut t.children[i];
        }
        t
    }

    /// The first open task with this text, in the order tasks are worked on.
    fn find(&self, task: &str) -> Option<Vec<usize>> {
        let want = norm(task);
        self.order()
            .into_iter()
            .find(|p| self.get(p).status.open() && norm(&self.get(p).text) == want)
    }

    /// Adds `steps` under `task` and marks it split. A task that isn't in the
    /// outline is added at the top level first.
    pub fn add_steps(&mut self, task: &str, steps: &[String]) {
        // No steps changes nothing: the task stays as it was, so it can
        // still be worked on and marked.
        if steps.is_empty() {
            return;
        }
        let path = match self.find(task) {
            Some(p) => p,
            None => {
                self.roots.push(Task {
                    text: task.to_string(),
                    ..Task::default()
                });
                vec![self.roots.len() - 1]
            }
        };
        let t = self.get_mut(&path);
        t.status = TaskStatus::Split;
        t.children.extend(steps.iter().map(|s| Task {
            text: s.clone(),
            ..Task::default()
        }));
    }

    /// Sets the status of `task`. Returns false if it isn't in the outline.
    pub fn mark(&mut self, task: &str, status: TaskStatus) -> bool {
        match self.find(task) {
            Some(p) => {
                self.get_mut(&p).status = status;
                true
            }
            None => false,
        }
    }

    /// The outline as numbered, indented lines.
    pub fn render(&self) -> String {
        fn walk(t: &Task, number: &str, depth: usize, out: &mut Vec<String>) {
            out.push(format!(
                "{}{} {number} {}",
                "    ".repeat(depth),
                t.status.mark(),
                line(&t.text)
            ));
            for (i, c) in t.children.iter().enumerate() {
                walk(c, &format!("{number}{}.", i + 1), depth + 1, out);
            }
        }
        let mut out = Vec::new();
        for (i, t) in self.roots.iter().enumerate() {
            walk(t, &format!("{}.", i + 1), 0, &mut out);
        }
        out.join("\n")
    }

    /// (done, all) counted over tasks that aren't split.
    pub fn progress(&self) -> (usize, usize) {
        let mut done = 0;
        let mut all = 0;
        for p in self.order() {
            let t = self.get(&p);
            if t.status != TaskStatus::Split {
                all += 1;
                if matches!(t.status, TaskStatus::Done | TaskStatus::Failed) {
                    done += 1;
                }
            }
        }
        (done, all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_no_steps_leaves_the_task_alone() {
        let mut o = Outline::default();
        o.start("Goal");
        o.add_steps("Goal", &[]);
        assert_eq!(o.render(), "○ 1. Goal");
        assert!(o.mark("Goal", TaskStatus::Done));
        assert_eq!(o.render(), "✓ 1. Goal");
    }

    #[test]
    fn nests_and_marks_in_working_order() {
        let mut o = Outline::default();
        o.start("Plan a party");
        o.add_steps("Task: Plan a party", &["Guests".into(), "Food".into()]);
        o.add_steps("Food", &["Snacks".into(), "Cake".into()]);
        assert!(o.mark("Guests", TaskStatus::Done));
        assert!(o.mark("**Snacks**", TaskStatus::Done));
        assert!(!o.mark("Music", TaskStatus::Done));
        assert_eq!(
            o.render(),
            "▾ 1. Plan a party\n    ✓ 1.1. Guests\n    ▾ 1.2. Food\n        ✓ 1.2.1. Snacks\n        ○ 1.2.2. Cake"
        );
        assert_eq!(o.progress(), (2, 3));
    }

    #[test]
    fn same_text_matches_the_first_open_task() {
        let mut o = Outline::default();
        o.start("Goal");
        o.add_steps("Goal", &["Check".into(), "Check".into()]);
        o.mark("Check", TaskStatus::Done);
        o.mark("Check", TaskStatus::Failed);
        assert!(o.render().contains("✓ 1.1. Check"));
        assert!(o.render().contains("✗ 1.2. Check"));
    }

    #[test]
    fn unknown_tasks_join_at_the_top() {
        let mut o = Outline::default();
        o.start("Goal");
        o.add_steps("Something else", &["A".into()]);
        assert_eq!(o.render(), "○ 1. Goal\n▾ 2. Something else\n    ○ 2.1. A");
        let mut long = Outline::default();
        long.start("First line\nsecond line");
        assert_eq!(long.render(), "○ 1. First line …");
    }
}
