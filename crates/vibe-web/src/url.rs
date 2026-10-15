//! URL（`http:` と `file:` だけ）と相対 URL の解決（RFC 3986 §5.2）
//!
//! - `http://ホスト[:ポート]/パス[?問い合わせ][#断片]`。ホストは小文字にする。パスが無ければ `/`
//! - `file:///パス`（`file:/パス` も）と、`/` で始まるだけのもの（フロッピーのファイル）
//! - `https:` は `UrlError::Https`、ほかの形式（`mailto:`・`javascript:` など）は `UrlError::Unsupported`
//!
//! パスの中の空白・ASCII の外の文字は `%XX` にする（そのまま HTTP の要求に書けるように）。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: Scheme,
    /// 小文字。file では空
    pub host: String,
    pub port: u16,
    /// `/` で始まるパス（`?` からの問い合わせも含む）
    pub path: String,
    /// `#` の後ろ（無ければ None）
    pub fragment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    /// https:（TLS は扱わない）
    Https,
    /// http・file 以外の形式（小文字にした名前）
    Unsupported(String),
    /// ホストが無い・使えない文字がある
    BadHost,
    BadPort,
}

impl UrlError {
    pub fn message(&self) -> &'static str {
        match self {
            UrlError::Empty => "empty URL",
            UrlError::Https => "HTTPS is not supported",
            UrlError::Unsupported(_) => "unsupported kind of URL",
            UrlError::BadHost => "bad host name",
            UrlError::BadPort => "bad port number",
        }
    }
}

impl Url {
    /// 絶対の URL を読む（`/` で始まるだけのものはフロッピーのファイル）
    pub fn parse(s: &str) -> Result<Url, UrlError> {
        let s = clean(s);
        if s.is_empty() {
            return Err(UrlError::Empty);
        }
        if s.starts_with('/') && !s.starts_with("//") {
            let (path, fragment) = split_fragment(&s);
            return Ok(Url::file(path, fragment));
        }
        let Some((scheme, rest)) = split_scheme(&s) else {
            return Err(UrlError::Unsupported(String::new()));
        };
        match scheme.as_str() {
            "http" => {
                let rest = rest.strip_prefix("//").ok_or(UrlError::BadHost)?;
                parse_authority_and_path(rest)
            }
            "file" => {
                // file:///PATH・file:/PATH・file://localhost/PATH
                let rest = match rest.strip_prefix("//") {
                    Some(r) => &r[r.find('/').unwrap_or(r.len())..],
                    None => rest,
                };
                let (path, fragment) = split_fragment(rest);
                let path = if path.starts_with('/') { remove_dots(path) } else { ["/", path].concat() };
                Ok(Url::file(&path, fragment))
            }
            "https" => Err(UrlError::Https),
            _ => Err(UrlError::Unsupported(scheme)),
        }
    }

    fn file(path: &str, fragment: Option<&str>) -> Url {
        Url {
            scheme: Scheme::File,
            host: String::new(),
            port: 0,
            path: remove_dots(path),
            fragment: fragment.map(String::from),
        }
    }

    /// この URL を元に `href` を解決する（RFC 3986 §5.2.2）
    pub fn join(&self, href: &str) -> Result<Url, UrlError> {
        let r = clean(href);
        if split_scheme(&r).is_some() {
            return Url::parse(&r);
        }
        if let Some(rest) = r.strip_prefix("//") {
            return match self.scheme {
                Scheme::Http => parse_authority_and_path(rest),
                Scheme::File => Url::parse(&["file://", rest].concat()),
            };
        }
        let (rest, fragment) = split_fragment(&r);
        let (rpath, rquery) = match rest.find('?') {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        let (bpath, bquery) = match self.path.find('?') {
            Some(i) => (&self.path[..i], &self.path[i..]),
            None => (self.path.as_str(), ""),
        };
        let path = if rpath.is_empty() {
            [bpath, rquery.unwrap_or(bquery)].concat()
        } else {
            let merged = if rpath.starts_with('/') {
                String::from(rpath)
            } else {
                [&bpath[..bpath.rfind('/').map_or(0, |i| i + 1)], rpath].concat()
            };
            let merged = if merged.starts_with('/') { merged } else { ["/", &merged].concat() };
            [remove_dots(&merged).as_str(), rquery.unwrap_or("")].concat()
        };
        Ok(Url {
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
            path: encode_path(&path),
            fragment: fragment.map(String::from),
        })
    }

    /// 断片（`#…`）を除いて同じ文書か
    pub fn same_document(&self, other: &Url) -> bool {
        self.scheme == other.scheme && self.host == other.host && self.port == other.port && self.path == other.path
    }

    /// 断片を除いたもの
    pub fn without_fragment(&self) -> Url {
        Url { fragment: None, ..self.clone() }
    }

    /// file のパス（`?` からを除き、`%XX` を戻したもの）
    pub fn file_path(&self) -> String {
        let p = self.path.split('?').next().unwrap_or("");
        percent_decode(p)
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scheme {
            Scheme::Http => {
                write!(f, "http://{}", self.host)?;
                if self.port != 80 {
                    write!(f, ":{}", self.port)?;
                }
            }
            Scheme::File => f.write_str("file://")?,
        }
        f.write_str(&self.path)?;
        if let Some(frag) = &self.fragment {
            write!(f, "#{frag}")?;
        }
        Ok(())
    }
}

/// 前後の空白を除き、中のタブと改行を消す（ブラウザと同じ）
fn clean(s: &str) -> String {
    s.trim_matches(|c: char| c.is_ascii_whitespace() || c.is_control())
        .chars()
        .filter(|&c| c != '\t' && c != '\n' && c != '\r')
        .collect()
}

/// 先頭の「名前:」（英字で始まり、英数字と + - . だけ）を小文字で取り出す
fn split_scheme(s: &str) -> Option<(String, &str)> {
    let i = s.find(':')?;
    let name = &s[..i];
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then(|| (name.to_ascii_lowercase(), &s[i + 1..]))
}

fn split_fragment(s: &str) -> (&str, Option<&str>) {
    match s.find('#') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    }
}

/// `//` の後ろ（ホスト[:ポート] とパス）
fn parse_authority_and_path(rest: &str) -> Result<Url, UrlError> {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // 利用者の名前（user@）は使わないので捨てる
    let authority = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, "")) => (h, 80),
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|&p| p != 0).ok_or(UrlError::BadPort)?),
        None => (authority, 80),
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) {
        return Err(UrlError::BadHost);
    }
    let (rest, fragment) = split_fragment(&rest[end..]);
    let path = if rest.starts_with('/') {
        match rest.find('?') {
            Some(i) => [remove_dots(&rest[..i]).as_str(), &rest[i..]].concat(),
            None => remove_dots(rest),
        }
    } else {
        ["/", rest].concat()
    };
    Ok(Url {
        scheme: Scheme::Http,
        host: host.to_ascii_lowercase(),
        port,
        path: encode_path(&path),
        fragment: fragment.map(String::from),
    })
}

/// `.` と `..` の区切りを取り除く（RFC 3986 §5.2.4）。`path` は `/` で始まる
pub fn remove_dots(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut trailing = false;
    for seg in path.split('/').skip(1) {
        trailing = false;
        match seg {
            "." => trailing = true,
            ".." => {
                out.pop();
                trailing = true;
            }
            s => out.push(s),
        }
    }
    let mut s = String::from("/");
    s.push_str(&out.join("/"));
    if trailing && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// 空白・制御文字・ASCII の外の文字などを `%XX` にする（`%` はそのまま）
fn encode_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b <= b' ' || b >= 0x7F || matches!(b, b'"' | b'<' | b'>' | b'`' | b'{' | b'}' | b'|' | b'\\' | b'^') {
            s.push('%');
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 15) as usize] as char);
        } else {
            s.push(b as char);
        }
    }
    s
}

/// `%XX` を戻す（形がおかしいものはそのまま。戻した結果が UTF-8 でなければ置き換え文字）
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
