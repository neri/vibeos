use crate::url::{Scheme, Url, UrlError};

fn join(base: &str, href: &str) -> String {
    Url::parse(base).unwrap().join(href).unwrap().to_string()
}

#[test]
fn parse_http() {
    let u = Url::parse("http://Example.COM:8080/a/b?x=1#top").unwrap();
    assert_eq!(u.scheme, Scheme::Http);
    assert_eq!(u.host, "example.com");
    assert_eq!(u.port, 8080);
    assert_eq!(u.path, "/a/b?x=1");
    assert_eq!(u.fragment.as_deref(), Some("top"));
    assert_eq!(u.to_string(), "http://example.com:8080/a/b?x=1#top");
    assert_eq!(Url::parse("http://example.com").unwrap().to_string(), "http://example.com/");
    assert_eq!(Url::parse("  http://a/b c  ").unwrap().path, "/b%20c");
    assert_eq!(Url::parse("http://user@a:80/").unwrap().to_string(), "http://a/");
}

#[test]
fn parse_file_and_errors() {
    let u = Url::parse("/DOCS/WEB/INDEX.HTM#x").unwrap();
    assert_eq!(u.scheme, Scheme::File);
    assert_eq!(u.path, "/DOCS/WEB/INDEX.HTM");
    assert_eq!(u.to_string(), "file:///DOCS/WEB/INDEX.HTM#x");
    assert_eq!(Url::parse("file:///DOCS/A.HTM").unwrap().path, "/DOCS/A.HTM");
    assert_eq!(Url::parse("file:/DOCS/../A.HTM").unwrap().path, "/A.HTM");
    assert_eq!(Url::parse("https://example.com/"), Err(UrlError::Https));
    assert_eq!(Url::parse("mailto:a@b"), Err(UrlError::Unsupported("mailto".into())));
    assert_eq!(Url::parse(""), Err(UrlError::Empty));
    assert_eq!(Url::parse("http://"), Err(UrlError::BadHost));
    assert_eq!(Url::parse("http://a:99999/"), Err(UrlError::BadPort));
    assert_eq!(Url::parse("http://a b/"), Err(UrlError::BadHost));
    assert!(Url::parse("example.com").is_err());
}

/// RFC 3986 §5.4.1・§5.4.2 の例（g:h は扱わない形式なので除く。//g はパスの / を付ける）
#[test]
fn rfc3986_examples() {
    let base = "http://a/b/c/d;p?q";
    let cases = [
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g/"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("g#s", "http://a/b/c/g#s"),
        ("g?y#s", "http://a/b/c/g?y#s"),
        (";x", "http://a/b/c/;x"),
        ("g;x", "http://a/b/c/g;x"),
        ("g;x?y#s", "http://a/b/c/g;x?y#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("./", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../..", "http://a/"),
        ("../../", "http://a/"),
        ("../../g", "http://a/g"),
        ("../../../g", "http://a/g"),
        ("../../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("/../g", "http://a/g"),
        ("g.", "http://a/b/c/g."),
        (".g", "http://a/b/c/.g"),
        ("g..", "http://a/b/c/g.."),
        ("..g", "http://a/b/c/..g"),
        ("./../g", "http://a/b/g"),
        ("./g/.", "http://a/b/c/g/"),
        ("g/./h", "http://a/b/c/g/h"),
        ("g/../h", "http://a/b/c/h"),
        ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g?y/./x", "http://a/b/c/g?y/./x"),
        ("g?y/../x", "http://a/b/c/g?y/../x"),
        ("g#s/./x", "http://a/b/c/g#s/./x"),
        ("g#s/../x", "http://a/b/c/g#s/../x"),
    ];
    for (href, want) in cases {
        assert_eq!(join(base, href), want, "href {href:?}");
    }
}

#[test]
fn join_other_cases() {
    assert_eq!(join("http://a/b/c", "http://x/y"), "http://x/y");
    assert_eq!(join("http://a/b/c", "HTTP://X/y"), "http://x/y");
    assert_eq!(join("http://a:8080/b/c", "d"), "http://a:8080/b/d");
    assert_eq!(join("http://a/b/c", " d e "), "http://a/b/d%20e");
    assert_eq!(join("http://a/b/c", "d\n/e"), "http://a/b/d/e");
    assert_eq!(join("/DOCS/WEB/INDEX.HTM", "PAGE2.HTM"), "file:///DOCS/WEB/PAGE2.HTM");
    assert_eq!(join("/DOCS/WEB/INDEX.HTM", "../README.TXT"), "file:///DOCS/README.TXT");
    assert_eq!(join("/DOCS/WEB/INDEX.HTM", "/HELLO.TXT"), "file:///HELLO.TXT");
    let base = Url::parse("http://a/b").unwrap();
    assert_eq!(base.join("https://a/"), Err(UrlError::Https));
    assert_eq!(base.join("javascript:void(0)"), Err(UrlError::Unsupported("javascript".into())));
    let u = base.join("#x").unwrap();
    assert!(u.same_document(&base));
    assert_eq!(u.without_fragment(), base);
}

#[test]
fn file_path_decodes() {
    let u = Url::parse("file:///DOCS/MY%20FILE.HTM?x").unwrap();
    assert_eq!(u.file_path(), "/DOCS/MY FILE.HTM");
    assert_eq!(crate::url::percent_decode("%4"), "%4");
    assert_eq!(crate::url::percent_decode("a%zz%41"), "a%zzA");
}
