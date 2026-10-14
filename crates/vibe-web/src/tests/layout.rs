use super::{Fixed, lay, lines_text};
use crate::doc::parse;
use crate::layout::{Font, MARGIN, PieceKind, layout};

#[test]
fn wraps_at_word_boundaries() {
    // 幅 116 → 文字を置ける幅は 100（12 文字）
    let l = lay("<p>The quick brown fox   jumps\n over the lazy dog.</p>", 116);
    assert_eq!(lines_text(&l), ["The quick", "brown fox", "jumps over", "the lazy", "dog."]);
    assert_eq!(l.lines[0].y, MARGIN);
    assert_eq!(l.lines[1].y, MARGIN + 16);
    assert_eq!(l.height, MARGIN + 5 * 16 + MARGIN);
    let p = &l.lines[0].pieces[0];
    assert_eq!((p.x, p.w), (MARGIN, 9 * 8));
}

#[test]
fn long_words_are_split() {
    let l = lay("ab abcdefghijklmnopqrstuvwxyz", 116);
    assert_eq!(lines_text(&l), ["ab", "abcdefghijkl", "mnopqrstuvwx", "yz"]);
}

#[test]
fn nbsp_does_not_break() {
    let l = lay("aaaa bbbb&nbsp;cccc", 116);
    assert_eq!(lines_text(&l), ["aaaa", "bbbb cccc"]);
}

#[test]
fn paragraph_gaps_collapse() {
    // 段落の間は GAP（16 * 3/4 = 12）。続いた間は大きい方だけ
    let l = lay("<p>a</p><p>b</p><div><p>c</p></div>d", 200);
    assert_eq!(lines_text(&l), ["a", "b", "c", "d"]);
    let ys: Vec<i32> = l.lines.iter().map(|l| l.y).collect();
    assert_eq!(ys, [8, 8 + 16 + 12, 8 + 2 * (16 + 12), 8 + 3 * (16 + 12)]);
}

#[test]
fn styles_make_separate_pieces() {
    let l = lay("a <b>bold</b> <a href=x>link here</a>.", 400);
    let line = &l.lines[0];
    let kinds: Vec<(i32, &str, Font, Option<u32>)> = line
        .pieces
        .iter()
        .map(|p| match &p.kind {
            PieceKind::Text { text, font, link } => (p.x, text.as_str(), *font, *link),
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (8, "a", Font::Normal, None),
            (24, "bold", Font::Bold, None),
            (64, "link here", Font::Normal, Some(0)),
            (136, ".", Font::Normal, None),
        ]
    );
    assert_eq!(l.link_at(70, 10), Some(0));
    assert_eq!(l.link_at(30, 10), None);
    assert_eq!(l.link_at(70, 100), None);
}

#[test]
fn headings_use_the_big_font() {
    let l = lay("<h1>Title</h1><h3>Sub</h3>text", 400);
    let fonts: Vec<Font> = l
        .lines
        .iter()
        .map(|line| match &line.pieces[0].kind {
            PieceKind::Text { font, .. } => *font,
            _ => panic!(),
        })
        .collect();
    assert_eq!(fonts, [Font::Heading, Font::Bold, Font::Normal]);
    assert_eq!((l.lines[0].height, l.lines[0].baseline), (24, 18));
}

#[test]
fn pre_keeps_spaces_and_tabs() {
    let l = lay("<pre>\na  b\n\tc\n\nd</pre>e", 400);
    assert_eq!(lines_text(&l), ["a  b", "        c", "", "d", "e"]);
    let font = match &l.lines[0].pieces[0].kind {
        PieceKind::Text { font, .. } => *font,
        _ => panic!(),
    };
    assert_eq!(font, Font::Mono);
}

#[test]
fn pre_wraps_long_lines() {
    let l = lay("<pre>0123456789abcdef</pre>", 116);
    assert_eq!(lines_text(&l), ["0123456789ab", "cdef"]);
}

#[test]
fn lists_indent_and_markers() {
    let l = lay("<ul><li>one<li>two<ul><li>sub</ul></ul><ol><li>x</ol>", 400);
    assert_eq!(lines_text(&l), ["* one", "* two", "* sub", "1. x"]);
    let text_x = |i: usize| l.lines[i].pieces[1].x;
    assert_eq!(text_x(0), MARGIN + 28);
    assert_eq!(text_x(2), MARGIN + 56);
    // 印は字下げの左
    assert!(l.lines[0].pieces[0].x < MARGIN + 28);
    assert!(matches!(l.lines[2].pieces[0].kind, PieceKind::Bullet(1)));
    assert!(l.lines[3].pieces[0].x + l.lines[3].pieces[0].w <= MARGIN + 28);
}

#[test]
fn blockquote_and_rule() {
    let l = lay("a<blockquote>q</blockquote><hr>b", 400);
    assert_eq!(lines_text(&l), ["a", "q", "---", "b"]);
    assert_eq!(l.lines[1].pieces[0].x, MARGIN + 28);
    let rule = &l.lines[2].pieces[0];
    assert_eq!((rule.x, rule.w), (MARGIN, 400 - 2 * MARGIN));
}

#[test]
fn table_rows_and_cells() {
    let l = lay("<table><tr><td>a</td><td>bb</td></tr><tr><td>c</td></tr></table>", 400);
    assert_eq!(lines_text(&l), ["a bb", "c"]);
    let row = &l.lines[0].pieces;
    assert_eq!(row[1].x, MARGIN + 8 + 20);
}

#[test]
fn breaks_make_empty_lines() {
    let l = lay("a<br>b<br><br>c", 400);
    assert_eq!(lines_text(&l), ["a", "b", "", "c"]);
}

#[test]
fn images_with_and_without_size() {
    let doc = parse("x <img src=a alt=Cat> <img src=b> <a href=l><img src=c></a>");
    let l = layout(&doc, 400, &Fixed, &|_| None);
    assert_eq!(lines_text(&l), ["x [Cat] [image] [image]"]);
    assert_eq!(l.link_at(l.lines[0].pieces.last().unwrap().x + 1, 10), Some(0));
    // 大きさが分かれば画像の部品。幅に入らなければ縮める
    let l = layout(&doc, 400, &Fixed, &|i| Some(if i == 1 { (768, 100) } else { (20, 30) }));
    assert_eq!(lines_text(&l), ["x <img 0>", "<img 1>", "<img 2>"]);
    // 文字の下の分（16 - 12）が画像の下（2）より大きい
    assert_eq!((l.lines[0].height, l.lines[0].baseline), (34, 30));
    let big = &l.lines[1].pieces[0];
    assert_eq!((big.w, big.h), (384, 50));
    assert_eq!(l.link_at(MARGIN + 1, l.lines[2].y + 1), Some(0));
}

#[test]
fn anchors_record_y() {
    let l = lay("<p>a</p><h2 id=two>b</h2>", 400);
    assert_eq!(l.anchor("two"), Some(l.lines[1].y));
    assert_eq!(l.anchor("none"), None);
}

#[test]
fn line_lookup() {
    let l = lay("a<br>b<br>c", 400);
    assert_eq!(l.line_at(MARGIN), Some(0));
    assert_eq!(l.line_at(MARGIN + 16), Some(1));
    assert_eq!(l.line_at(0), None);
    assert_eq!(l.line_at(1000), None);
    assert_eq!(l.first_visible(MARGIN + 20), 1);
}
