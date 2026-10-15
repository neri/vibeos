use crate::doc::{Block, Item, Marker, Style, parse, plain};

fn t(s: &str) -> Item {
    Item::Text(s.into(), Style::default())
}

fn styled(s: &str, f: impl FnOnce(&mut Style)) -> Item {
    let mut st = Style::default();
    f(&mut st);
    Item::Text(s.into(), st)
}

#[test]
fn title_head_and_hidden_content() {
    let d = parse(
        "<html><head><title> My\n  Page </title><style>p{color:red}</style>\
         <script>document.write('x')</script><base href='http://b/'></head>\
         <body>Hi<select><option>no</select><noscript>ns</noscript></body></html>",
    );
    assert_eq!(d.title, "My Page");
    assert_eq!(d.base.as_deref(), Some("http://b/"));
    assert_eq!(
        d.items,
        [Item::Open(Block::Div), t("Hi[...]"), Item::Open(Block::Div), t("ns"), Item::Close, Item::Close]
    );
}

#[test]
fn styles_and_links() {
    let d = parse("a<b>b<i>c</i></b><code>d</code><a href='x.htm'>e<strong>f</strong></a><a name=n>g</a>");
    assert_eq!(d.links, ["x.htm"]);
    assert_eq!(
        d.items,
        [
            t("a"),
            styled("bc", |s| s.bold = true),
            styled("d", |s| s.mono = true),
            styled("e", |s| s.link = Some(0)),
            styled("f", |s| {
                s.link = Some(0);
                s.bold = true
            }),
            Item::Anchor("n".into()),
            t("g"),
        ]
    );
}

#[test]
fn implied_closes() {
    // <p> は次のブロックで閉じる。<li> は前の <li> を閉じる
    let d = parse("<p>one<p>two<ul><li>a<li>b</ul><h1>x<h2>y</h2>");
    assert_eq!(
        d.items,
        [
            Item::Open(Block::Para),
            t("one"),
            Item::Close,
            Item::Open(Block::Para),
            t("two"),
            Item::Close,
            Item::Open(Block::List),
            Item::Open(Block::Item(Marker::Bullet(0))),
            t("a"),
            Item::Close,
            Item::Open(Block::Item(Marker::Bullet(0))),
            t("b"),
            Item::Close,
            Item::Close,
            Item::Open(Block::Heading(1)),
            styled("x", |s| s.heading = 1),
            Item::Close,
            Item::Open(Block::Heading(2)),
            styled("y", |s| s.heading = 2),
            Item::Close,
        ]
    );
}

#[test]
fn table_cells_close_each_other() {
    let d = parse("<table><tr><th>A<td>B<tr><td>C</table>after");
    assert_eq!(
        d.items,
        [
            Item::Open(Block::Table),
            Item::Open(Block::Row),
            Item::Open(Block::Cell),
            styled("A", |s| s.bold = true),
            Item::Close,
            Item::Open(Block::Cell),
            t("B"),
            Item::Close,
            Item::Close,
            Item::Open(Block::Row),
            Item::Open(Block::Cell),
            t("C"),
            Item::Close,
            Item::Close,
            Item::Close,
            t("after"),
        ]
    );
}

#[test]
fn ordered_lists_and_nesting() {
    let d = parse("<ol start=3><li>a<li value=10>b<li>c<ul><li>d</ul></ol>");
    let markers: Vec<Marker> = d
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Open(Block::Item(m)) => Some(*m),
            _ => None,
        })
        .collect();
    assert_eq!(markers, [Marker::Number(3), Marker::Number(10), Marker::Number(11), Marker::Bullet(1)]);
}

#[test]
fn misnested_and_stray_end_tags() {
    // </b> は中の <i> も閉じる。対応の無い終了タグは捨てる
    let d = parse("<b>1<i>2</b>3</i></div></p>4");
    assert_eq!(d.items, [styled("12", |s| s.bold = true), t("34")]);
    // 閉じないまま終わっても Open と Close の数がそろう
    let d = parse("<div><blockquote><pre>x");
    let opens = d.items.iter().filter(|i| matches!(i, Item::Open(_))).count();
    let closes = d.items.iter().filter(|i| matches!(i, Item::Close)).count();
    assert_eq!((opens, closes), (3, 3));
}

#[test]
fn void_elements() {
    let d = parse(
        "a<br>b<hr><img src=p.png alt='A  pic' width=10 height='20px'>\
         <input type=submit value=Go><input type=hidden value=x><input name=q><input type=checkbox checked>c</br>",
    );
    assert_eq!(
        d.items,
        [t("a"), Item::Break, t("b"), Item::Rule, Item::Image(0), t("[Go][________][x]c"), Item::Break,]
    );
    let img = &d.images[0];
    assert_eq!((img.src.as_str(), img.alt.as_str(), img.width, img.height), ("p.png", "A pic", Some(10), Some(20)));
}

#[test]
fn ids_become_anchors() {
    let d = parse("<h2 id=intro>Intro</h2><div id=''>x</div>");
    assert_eq!(d.items[0], Item::Anchor("intro".into()));
    assert!(!d.items.iter().any(|i| *i == Item::Anchor(String::new())));
}

#[test]
fn text_is_folded() {
    let d = parse("caf&eacute; &ldquo;x&rdquo;&nbsp;y");
    assert_eq!(d.items, [t("caf? \"x\"\u{a0}y")]);
}

#[test]
fn plain_text() {
    let d = plain("a\tb\n");
    assert_eq!(d.items, [Item::Open(Block::Pre), styled("a\tb\n", |s| s.mono = true), Item::Close]);
}
