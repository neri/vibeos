use super::*;

fn argv(p: &Pipeline, i: usize) -> Vec<&str> {
    p.commands[i].argv.iter().map(String::as_str).collect()
}

fn one(line: &str) -> Pipeline {
    parse(line).unwrap().unwrap()
}

#[test]
fn words_and_spaces() {
    let p = one("  ls   -l  /BIN ");
    assert_eq!(p.commands.len(), 1);
    assert_eq!(argv(&p, 0), ["ls", "-l", "/BIN"]);
    assert!(!p.background);
    assert_eq!(parse("   ").unwrap(), None);
    assert_eq!(parse("").unwrap(), None);
}

#[test]
fn quotes() {
    assert_eq!(argv(&one(r#"echo "two  words" 'a|b>c' x"y"z"#), 0), ["echo", "two  words", "a|b>c", "xyz"]);
    assert_eq!(argv(&one(r#"echo "" ''"#), 0), ["echo", "", ""]);
    assert_eq!(argv(&one(r#"echo "say \"hi\" \\ \n""#), 0), ["echo", r#"say "hi" \ \n"#]);
    assert_eq!(argv(&one(r"echo a\ b \| \'"), 0), ["echo", "a b", "|", "'"]);
    assert_eq!(argv(&one(r#"echo 'it''s'"#), 0), ["echo", "its"]);
    assert_eq!(parse(r#"echo "abc"#), Err(ParseError::UnterminatedQuote('"')));
    assert_eq!(parse("echo 'abc"), Err(ParseError::UnterminatedQuote('\'')));
    assert_eq!(parse("echo abc\\"), Err(ParseError::TrailingBackslash));
}

#[test]
fn pipes() {
    let p = one("ls /BIN | grep ELF|wc -l");
    assert_eq!(p.commands.len(), 3);
    assert_eq!(argv(&p, 0), ["ls", "/BIN"]);
    assert_eq!(argv(&p, 1), ["grep", "ELF"]);
    assert_eq!(argv(&p, 2), ["wc", "-l"]);
    assert_eq!(parse("| wc"), Err(ParseError::EmptyCommand));
    assert_eq!(parse("ls |"), Err(ParseError::EmptyCommand));
    assert_eq!(parse("ls || wc"), Err(ParseError::EmptyCommand));
}

#[test]
fn redirects() {
    let p = one("cat < IN.TXT > OUT.TXT");
    assert_eq!(argv(&p, 0), ["cat"]);
    assert_eq!(
        p.commands[0].redirects,
        [Redirect { fd: 0, target: Target::Read("IN.TXT".into()) }, Redirect { fd: 1, target: Target::Write("OUT.TXT".into()) }]
    );
    let p = one("cmd >>LOG.TXT 2>&1");
    assert_eq!(
        p.commands[0].redirects,
        [Redirect { fd: 1, target: Target::Append("LOG.TXT".into()) }, Redirect { fd: 2, target: Target::Dup(1) }]
    );
    let p = one("cmd 2> ERR.TXT arg 1>&2");
    assert_eq!(argv(&p, 0), ["cmd", "arg"]);
    assert_eq!(
        p.commands[0].redirects,
        [Redirect { fd: 2, target: Target::Write("ERR.TXT".into()) }, Redirect { fd: 1, target: Target::Dup(2) }]
    );
    // 語の途中の数字は fd ではない
    let p = one("echo a2>F");
    assert_eq!(argv(&p, 0), ["echo", "a2"]);
    assert_eq!(p.commands[0].redirects, [Redirect { fd: 1, target: Target::Write("F".into()) }]);
    // 引用符の中の記号はただの文字。ファイル名も引用できる
    let p = one(r#"echo ">" > "A B.TXT""#);
    assert_eq!(argv(&p, 0), ["echo", ">"]);
    assert_eq!(p.commands[0].redirects, [Redirect { fd: 1, target: Target::Write("A B.TXT".into()) }]);
}

#[test]
fn redirect_errors() {
    assert_eq!(parse("ls >"), Err(ParseError::MissingTarget(">")));
    assert_eq!(parse("ls > | wc"), Err(ParseError::MissingTarget(">")));
    assert_eq!(parse("ls >>"), Err(ParseError::MissingTarget(">>")));
    assert_eq!(parse("cat <"), Err(ParseError::MissingTarget("<")));
    assert_eq!(parse("ls 2>&x"), Err(ParseError::MissingTarget(">&")));
    assert_eq!(parse("ls 2>&7"), Err(ParseError::BadFd(7)));
    assert_eq!(parse("ls 5> F"), Err(ParseError::BadFd(5)));
    assert_eq!(parse("> F"), Err(ParseError::EmptyCommand));
}

#[test]
fn background() {
    let p = one("httpd 8080 &");
    assert!(p.background);
    assert_eq!(argv(&p, 0), ["httpd", "8080"]);
    let p = one("a | b&");
    assert!(p.background);
    assert_eq!(p.commands.len(), 2);
    assert_eq!(parse("a & b"), Err(ParseError::MisplacedAmpersand));
    assert_eq!(parse("a && b"), Err(ParseError::MisplacedAmpersand));
    assert_eq!(parse("&"), Err(ParseError::EmptyCommand));
}

#[test]
fn error_messages() {
    assert_eq!(ParseError::MissingTarget(">").to_string(), "missing file name after >");
    assert_eq!(ParseError::UnterminatedQuote('"').to_string(), "missing closing \"");
}
