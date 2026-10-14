use crate::http::{HttpError, Reader, request};
use crate::url::Url;

/// `data` を `step` バイトずつ渡して読む
fn read_in_steps(data: &[u8], step: usize, limit: usize) -> Result<(crate::http::Head, Vec<u8>, bool), HttpError> {
    let mut r = Reader::new(limit);
    for part in data.chunks(step.max(1)) {
        r.feed(part)?;
    }
    r.finish()
}

#[test]
fn builds_the_request() {
    let u = Url::parse("http://Example.com:8080/a/b?x=1#frag").unwrap();
    let req = String::from_utf8(request(&u)).unwrap();
    assert!(req.starts_with("GET /a/b?x=1 HTTP/1.0\r\nHost: example.com:8080\r\n"), "{req}");
    assert!(req.contains("User-Agent: VibeOS-browse/1.0\r\n"));
    assert!(req.ends_with("Connection: close\r\n\r\n"));
    let req = String::from_utf8(request(&Url::parse("http://a/").unwrap())).unwrap();
    assert!(req.contains("Host: a\r\n"));
}

#[test]
fn content_length_in_any_pieces() {
    let resp = b"HTTP/1.1 200 OK\r\nContent-Type: Text/HTML; charset=ISO-8859-1\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
    for step in 1..resp.len() {
        let (h, body, cut) = read_in_steps(resp, step, 1000).unwrap();
        assert_eq!((h.status, h.reason.as_str()), (200, "OK"));
        assert_eq!(h.mime.as_deref(), Some("text/html"));
        assert_eq!(h.charset.as_deref(), Some("ISO-8859-1"));
        assert_eq!((body.as_slice(), cut), (&b"hello"[..], false));
    }
}

#[test]
fn done_when_length_reached() {
    let mut r = Reader::new(100);
    r.feed(b"HTTP/1.0 200 OK\r\nContent-Length: 3\r\n\r\nab").unwrap();
    assert!(!r.done());
    r.feed(b"c").unwrap();
    assert!(r.done());
    assert_eq!(r.received(), 3);
}

#[test]
fn chunked_in_any_pieces() {
    let resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n6;ext=1\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\nX-Trailer: 1\r\n\r\n";
    for step in 1..resp.len() {
        let mut r = Reader::new(1000);
        for part in resp.chunks(step) {
            r.feed(part).unwrap();
        }
        assert!(r.done(), "step {step}");
        let (h, body, _) = r.finish().unwrap();
        assert!(h.chunked);
        assert_eq!(String::from_utf8(body).unwrap(), "Wikipedia in \r\n\r\nchunks.");
    }
}

#[test]
fn bad_chunks() {
    assert_eq!(
        read_in_steps(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 100, 100).unwrap_err(),
        HttpError::BadChunk
    );
    assert_eq!(
        read_in_steps(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabc\r\n", 100, 100).unwrap_err(),
        HttpError::BadChunk
    );
    // 最後の塊の前に閉じた
    assert_eq!(
        read_in_steps(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n", 100, 100).unwrap_err(),
        HttpError::Incomplete
    );
}

#[test]
fn body_until_close_and_limit() {
    let (h, body, cut) = read_in_steps(b"HTTP/1.0 200 OK\n\nno length here", 4, 100).unwrap();
    assert_eq!((h.status, h.length), (200, None));
    assert_eq!((body.as_slice(), cut), (&b"no length here"[..], false));
    // 上限で切る
    let (_, body, cut) = read_in_steps(b"HTTP/1.0 200 OK\r\n\r\n0123456789", 3, 4).unwrap();
    assert_eq!((body.as_slice(), cut), (&b"0123"[..], true));
    // Content-Length より前に閉じた
    assert_eq!(
        read_in_steps(b"HTTP/1.0 200 OK\r\nContent-Length: 10\r\n\r\nabc", 100, 100).unwrap_err(),
        HttpError::Incomplete
    );
}

#[test]
fn redirects_and_errors() {
    let (h, _, _) =
        read_in_steps(b"HTTP/1.1 301 Moved Permanently\r\nLocation: /new\r\nContent-Length: 0\r\n\r\n", 7, 10).unwrap();
    assert!(h.is_redirect());
    assert_eq!(h.location.as_deref(), Some("/new"));
    let (h, _, _) = read_in_steps(b"HTTP/1.1 302 Found\r\n\r\n", 7, 10).unwrap();
    assert!(!h.is_redirect());
    assert_eq!(read_in_steps(b"SSH-2.0-OpenSSH\r\n\r\n", 100, 10).unwrap_err(), HttpError::Malformed);
    assert_eq!(read_in_steps(b"HTTP/1.1 abc OK\r\n\r\n", 100, 10).unwrap_err(), HttpError::Malformed);
    assert_eq!(read_in_steps(b"HTTP/1.1 200 OK\r\n", 100, 10).unwrap_err(), HttpError::Incomplete);
    let long = [b"HTTP/1.1 200 OK\r\nX: ".as_slice(), &[b'a'; 20000]].concat();
    assert_eq!(read_in_steps(&long, 1000, 10).unwrap_err(), HttpError::TooLong);
}
