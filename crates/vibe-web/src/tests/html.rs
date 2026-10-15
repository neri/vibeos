use crate::charset::{self, Charset};
use crate::html::{Token, Tokenizer, decode_entities};

fn tokens(s: &str) -> Vec<Token> {
    Tokenizer::new(s).collect()
}

fn start(name: &str, attrs: &[(&str, &str)]) -> Token {
    Token::Start {
        name: name.into(),
        attrs: attrs.iter().map(|&(k, v)| (k.into(), v.into())).collect(),
        self_closing: false,
    }
}

fn end(name: &str) -> Token {
    Token::End { name: name.into() }
}

fn text(s: &str) -> Token {
    Token::Text(s.into())
}

fn ent(s: &str) -> String {
    let mut out = String::new();
    decode_entities(s, false, &mut out);
    out
}

#[test]
fn tags_and_attributes() {
    assert_eq!(
        tokens(r#"<P Class="a b" ID=x data-y='1>2' checked>Hi</P>"#),
        [start("p", &[("class", "a b"), ("id", "x"), ("data-y", "1>2"), ("checked", "")]), text("Hi"), end("p"),]
    );
    assert_eq!(
        tokens("<br/><img src=a.png />"),
        [
            Token::Start { name: "br".into(), attrs: vec![], self_closing: true },
            Token::Start { name: "img".into(), attrs: vec![("src".into(), "a.png".into())], self_closing: true },
        ]
    );
    // 同じ名前の 2 つ目は捨てる
    assert_eq!(tokens("<a href=1 HREF=2>"), [start("a", &[("href", "1")])]);
}

#[test]
fn comments_doctype_and_stray_lt() {
    assert_eq!(tokens("<!DOCTYPE html><!-- <b> -->a < b <3 <?xml x?></ x>c"), [text("a < b <3 "), text("c")]);
    // 閉じないコメント・途中で切れたタグ
    assert_eq!(tokens("x<!-- y"), [text("x")]);
    assert_eq!(tokens("x<a href="), [text("x")]);
    assert_eq!(tokens("x</b"), [text("x")]);
    assert_eq!(tokens("<"), [text("<")]);
}

#[test]
fn raw_text_elements() {
    assert_eq!(
        tokens("<script>if (a<b && c>d) x='</p>';</script >after"),
        [start("script", &[]), text("if (a<b && c>d) x='</p>';"), end("script"), text("after")]
    );
    assert_eq!(tokens("<style>p{}</STYLE>"), [start("style", &[]), text("p{}"), end("style")]);
    // title は文字参照を戻す。タグは読まない
    assert_eq!(tokens("<title>A &amp; <b>B</title>"), [start("title", &[]), text("A & <b>B"), end("title")]);
    // 閉じないまま終わる
    assert_eq!(tokens("<script>abc"), [start("script", &[]), text("abc")]);
    assert_eq!(tokens("<script></script>"), [start("script", &[]), end("script")]);
}

#[test]
fn entities() {
    assert_eq!(ent("&amp;&lt;&gt;&quot;&apos;&nbsp;"), "&<>\"'\u{a0}");
    assert_eq!(ent("&#65;&#x42;&#X43;&#169;&#0;&#x110000;&#150;"), "ABC©\u{FFFD}\u{FFFD}–");
    assert_eq!(ent("&copy; &eacute; &hellip; &rarr;"), "© é … →");
    assert_eq!(ent("&amp &lt3 &notit; &nosuch; & &#; &#x;"), "& <3 ¬it; &nosuch; & &#; &#x;");
    let mut attr = String::new();
    decode_entities("?a=1&copy=2&amp;b&lt", true, &mut attr);
    assert_eq!(attr, "?a=1&copy=2&b<");
}

#[test]
fn charsets() {
    assert_eq!(charset::detect(b"abc", Some("ISO-8859-1")), Charset::Windows1252);
    assert_eq!(charset::detect(b"\xE9", Some("\"utf-8\"")), Charset::Utf8);
    assert_eq!(charset::detect(b"<meta charset=\"windows-1252\">\xC3\xA9", None), Charset::Windows1252);
    assert_eq!(
        charset::detect(b"<META HTTP-EQUIV=Content-Type CONTENT='text/html; charset=UTF-8'>", None),
        Charset::Utf8
    );
    assert_eq!(charset::detect("caf\u{e9}".as_bytes(), None), Charset::Utf8);
    assert_eq!(charset::detect(b"caf\xE9", None), Charset::Windows1252);
    assert_eq!(charset::label_in_content_type("text/html; charset=EUC-JP"), Some("EUC-JP"));
    assert_eq!(charset::label_in_content_type("text/html"), None);
    assert_eq!(charset::decode(b"\x93hi\x94 \xE9", Charset::Windows1252), "“hi” é");
    assert_eq!(charset::decode(b"\xEF\xBB\xBFa\xFF", Charset::Utf8), "a\u{FFFD}");
}

#[test]
fn fold_to_ascii() {
    assert_eq!(charset::fold("“Hi” — it’s 5°… ©2026 • a×b ¾"), "\"Hi\" - it's 5 deg... (c)2026 * axb 3/4");
    assert_eq!(charset::fold("caf\u{e9} 日本\u{200B}x\u{ad}y\u{a0}z"), "caf? ??xy\u{a0}z");
}
