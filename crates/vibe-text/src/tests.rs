use super::*;

fn opts(args: &[&str], known: &str, with_value: &str) -> Result<Options, OptError> {
    parse_options(args.iter().copied(), known, with_value)
}

#[test]
fn options() {
    let o = opts(&["-iv", "ELF", "-n", "A.TXT"], "ivn", "").unwrap();
    assert!(o.has('i') && o.has('v') && o.has('n'));
    assert_eq!(o.rest, ["ELF", "A.TXT"]);
    let o = opts(&["-n", "3", "F"], "n", "n").unwrap();
    assert_eq!(o.value('n'), Some("3"));
    assert_eq!(o.rest, ["F"]);
    let o = opts(&["-n5"], "n", "n").unwrap();
    assert_eq!(o.value('n'), Some("5"));
    let o = opts(&["-7", "--", "-x"], "n#", "n").unwrap();
    assert_eq!(o.value('#'), Some("7"));
    assert_eq!(o.rest, ["-x"]);
    // "-" だけは引数（標準入力を表すことがある）
    assert_eq!(opts(&["-"], "", "").unwrap().rest, ["-"]);
    assert_eq!(opts(&["-x"], "ab", ""), Err(OptError::Unknown('x')));
    assert_eq!(opts(&["-n"], "n", "n"), Err(OptError::MissingValue('n')));
    assert_eq!(opts(&["-5"], "n", "n"), Err(OptError::Unknown('5')));
}

#[test]
fn split_lines() {
    assert_eq!(lines("a\nb\r\n\nc").collect::<Vec<_>>(), ["a", "b", "", "c"]);
    assert_eq!(lines("a\n").collect::<Vec<_>>(), ["a"]);
    assert_eq!(lines("").count(), 0);
}

#[test]
fn grep() {
    let g = Grep { pattern: "ELF", ..Default::default() };
    assert!(g.matches(" HELLO.ELF  1234"));
    assert!(!g.matches(" hello.elf"));
    let g = Grep { pattern: "elf", ignore_case: true, ..Default::default() };
    assert!(g.matches(" HELLO.ELF"));
    let g = Grep { pattern: "ELF", invert: true, ..Default::default() };
    assert!(!g.matches("A.ELF"));
    assert!(g.matches("A.TXT"));
    assert!(Grep { pattern: "", ..Default::default() }.matches("anything"));
}

#[test]
fn counts() {
    assert_eq!(Counts::of(b"one two\nthree\n"), Counts { lines: 2, words: 3, bytes: 14 });
    assert_eq!(Counts::of(b"  x"), Counts { lines: 0, words: 1, bytes: 3 });
    // 語が 2 回に分かれて来ても 1 つと数える
    let mut c = Counts::default();
    let mut in_word = false;
    c.add(b"hel", &mut in_word);
    c.add(b"lo world\n", &mut in_word);
    assert_eq!(c, Counts { lines: 1, words: 2, bytes: 12 });
}

#[test]
fn head_and_tail() {
    let text = "1\n2\n3\n4\n5\n";
    assert_eq!(head(text, 3), ["1", "2", "3"]);
    assert_eq!(head(text, 10).len(), 5);
    assert_eq!(tail(text, 2), ["4", "5"]);
    assert_eq!(tail(text, 0).len(), 0);
    assert_eq!(tail(text, 9).len(), 5);
    let o = opts(&["-n", "4"], "n#", "n").unwrap();
    assert_eq!(line_count(&o), Ok(4));
    let o = opts(&["-3"], "n#", "n").unwrap();
    assert_eq!(line_count(&o), Ok(3));
    assert_eq!(line_count(&Options::default()), Ok(10));
    let o = opts(&["-n", "x"], "n#", "n").unwrap();
    assert_eq!(line_count(&o), Err(()));
}

#[test]
fn sort() {
    let l = vec!["banana", "apple", "cherry", "apple"];
    assert_eq!(Sort::default().sort(l.clone()), ["apple", "apple", "banana", "cherry"]);
    assert_eq!(Sort { reverse: true, unique: true, ..Default::default() }.sort(l), ["cherry", "banana", "apple"]);
    let n = vec!["10 x", "9", "-2", "abc", "100"];
    assert_eq!(Sort { numeric: true, ..Default::default() }.sort(n.clone()), ["-2", "abc", "9", "10 x", "100"]);
    assert_eq!(Sort::default().sort(n), ["-2", "10 x", "100", "9", "abc"]);
}

#[test]
fn merge_sort_matches_std() {
    let mut seed = 12345u32;
    for n in [0, 1, 2, 3, 7, 8, 9, 100, 257] {
        let mut v: Vec<(u32, usize)> = (0..n)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed % 10, i)
            })
            .collect();
        let mut want = v.clone();
        want.sort_by_key(|p| p.0); // 安定
        merge_sort(&mut v, |a, b| a.0 < b.0);
        assert_eq!(v, want, "n = {n}");
    }
}
