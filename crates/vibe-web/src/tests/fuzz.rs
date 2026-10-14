//! でたらめな入力でも panic しない（決まった種の xorshift32）

use super::Fixed;
use crate::{charset, doc, layout};

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.next() as usize % xs.len()]
    }
}

fn run(s: &str, width: i32) {
    let d = doc::parse(s);
    let l = layout::layout(&d, width, &Fixed, &|i| (i % 2 == 0).then_some((i * 37 % 500, 17)));
    // 行は上から順で、重ならない
    for w in l.lines.windows(2) {
        assert!(w[0].y + w[0].height <= w[1].y);
    }
}

#[test]
fn random_bytes() {
    let mut r = Rng(0x1234_5678);
    for n in 0..3000 {
        let len = r.next() % 200;
        let bytes: Vec<u8> = (0..len)
            .map(|_| match r.next() % 8 {
                0 => b'<',
                1 => b'>',
                2 => b'&',
                3 => b'/',
                _ => r.next() as u8,
            })
            .collect();
        let s =
            charset::decode(&bytes, if n % 2 == 0 { charset::Charset::Utf8 } else { charset::Charset::Windows1252 });
        run(&s, 40 + (r.next() % 400) as i32);
    }
}

#[test]
fn random_tag_soup() {
    const PARTS: &[&str] = &[
        "<p>",
        "</p>",
        "<li>",
        "</li>",
        "<ul>",
        "</ul>",
        "<ol start=-5>",
        "</ol>",
        "<table>",
        "<tr>",
        "<td>",
        "</td>",
        "</table>",
        "<pre>",
        "</pre>",
        "\n",
        "\t",
        " ",
        "word",
        "longlonglonglonglongword",
        "<b>",
        "</b>",
        "<a href=x>",
        "</a>",
        "<img src=y>",
        "<img alt=z>",
        "<br>",
        "<hr>",
        "<h1>",
        "</h2>",
        "<blockquote>",
        "<dd>",
        "<dt>",
        "<script>",
        "</script>",
        "<!--",
        "-->",
        "&amp;",
        "&#x41",
        "&",
        "<",
        "</",
        "<div id=a>",
        "</div>",
        "<title>",
        "</title>",
        "<select>",
        "\u{a0}",
        "日本",
        "<a",
        "=\"",
        "'",
    ];
    let mut r = Rng(42);
    for _ in 0..3000 {
        let n = r.next() % 60;
        let s: String = (0..n).map(|_| r.pick(PARTS)).collect();
        run(&s, 40 + (r.next() % 400) as i32);
    }
}

#[test]
fn deep_nesting() {
    let s = "<div><blockquote><ul><li>".repeat(400) + "x";
    run(&s, 300);
    let s = "<b>".repeat(10_000) + "x";
    run(&s, 300);
}
