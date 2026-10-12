use super::*;
use std::collections::VecDeque as Queue;

struct Fake {
    keys: Queue<Key>,
    out: String,
    clip: String,
}

impl Term for Fake {
    fn read_key(&mut self) -> Option<Key> {
        self.keys.pop_front()
    }
    fn write(&mut self, s: &str) {
        self.out.push_str(s);
    }
    fn clipboard(&mut self) -> String {
        self.clip.clone()
    }
}

fn keys(text: &str, extra: &[Key]) -> Fake {
    let mut k: Queue<Key> = text.chars().map(Key::Char).collect();
    k.extend(extra.iter().copied());
    Fake { keys: k, out: String::new(), clip: String::new() }
}

#[test]
fn type_and_enter() {
    let mut ed = LineEditor::new();
    let mut t = keys("echo hi", &[Key::Enter]);
    assert_eq!(ed.read_line(&mut t, "> "), Input::Line("echo hi".into()));
    assert_eq!(t.out, "> echo hi\n");
    assert_eq!(ed.history().collect::<Vec<_>>(), ["echo hi"]);
}

#[test]
fn insert_in_the_middle() {
    // "eco hi" → Home, →, → で 'c' の前に 'h'
    let mut ed = LineEditor::new();
    let mut t = keys("eco hi", &[Key::Home, Key::Right, Key::Right, Key::Char('h'), Key::Enter]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Line("echo hi".into()));
    // 挿入した文字から後ろを書き直して戻る
    assert!(t.out.contains("\x1b[6D\x1b[1C\x1b[1Cho hi\x1b[4D"), "{:?}", t.out);
}

#[test]
fn backspace_and_delete() {
    let mut ed = LineEditor::new();
    let mut t = keys("echo abcX", &[Key::Backspace, Key::Left, Key::Delete, Key::Enter]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Line("echo ab".into()));
    // 何も無いところでの BackSpace・Delete は何もしない
    let mut t = keys("", &[Key::Backspace, Key::Delete, Key::Left, Key::Right, Key::Enter]);
    assert_eq!(ed.read_line(&mut t, "$"), Input::Line("".into()));
    assert_eq!(t.out, "$\n");
}

#[test]
fn history_up_down() {
    let mut ed = LineEditor::new();
    for l in ["one", "two", "two"] {
        let mut t = keys(l, &[Key::Enter]);
        ed.read_line(&mut t, "");
    }
    // 同じ行が続けば 1 つ、空の行は入れない
    let mut t = keys("", &[Key::Enter]);
    ed.read_line(&mut t, "");
    assert_eq!(ed.history().collect::<Vec<_>>(), ["one", "two"]);
    let mut t = keys("dr", &[Key::Up, Key::Up, Key::Up, Key::Down, Key::Down, Key::Enter]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Line("dr".into()));
    // 今は one, two, dr
    let mut t = keys("", &[Key::Up, Key::Up, Key::Up, Key::Enter]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Line("one".into()));
}

#[test]
fn history_is_limited() {
    let mut ed = LineEditor::new();
    for i in 0..40 {
        let mut t = keys(&format!("c{i}"), &[Key::Enter]);
        ed.read_line(&mut t, "");
    }
    let h: Vec<_> = ed.history().cloned().collect();
    assert_eq!(h.len(), 32);
    assert_eq!(h[0], "c8");
}

#[test]
fn cancel_paste_redraw() {
    let mut ed = LineEditor::new();
    let mut t = keys("help", &[Key::Cancel]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Cancelled);
    assert!(t.out.ends_with("help^C\n"));
    assert_eq!(ed.history().count(), 0);
    // 貼り付けは 1 行目の表示できる文字だけ
    let mut t = keys("echo <", &[Key::Paste, Key::Char('>'), Key::Enter]);
    t.clip = "edited\tline\nsecond".into();
    assert_eq!(ed.read_line(&mut t, ""), Input::Line("echo <editedline>".into()));
    let mut t = keys("ab", &[Key::Left, Key::Redraw, Key::Enter]);
    assert_eq!(ed.read_line(&mut t, "P> "), Input::Line("ab".into()));
    assert!(t.out.contains("\x1b[2J\x1b[HP> ab\x1b[1D"), "{:?}", t.out);
    // 端末が閉じた
    let mut t = keys("x", &[]);
    assert_eq!(ed.read_line(&mut t, ""), Input::Cancelled);
}

#[test]
fn line_length_is_limited() {
    let mut ed = LineEditor::new();
    let long: String = core::iter::repeat_n('a', 300).collect();
    let mut t = keys(&long, &[Key::Enter]);
    let Input::Line(l) = ed.read_line(&mut t, "") else { panic!() };
    assert_eq!(l.len(), 256);
}

#[test]
fn from_char() {
    assert_eq!(Key::from_char(Some('\n')), Key::Enter);
    assert_eq!(Key::from_char(Some('\x08')), Key::Backspace);
    assert_eq!(Key::from_char(Some('\x03')), Key::Cancel);
    assert_eq!(Key::from_char(Some('a')), Key::Char('a'));
    assert_eq!(Key::from_char(Some('\x01')), Key::Other);
    assert_eq!(Key::from_char(None), Key::Other);
}
