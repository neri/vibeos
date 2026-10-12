//! TCP の接続 1 本分（RFC 793 / 6298 / 5681 を簡単にしたもの）
//!
//! - 送信: 送るデータは `send_buf`（先頭が snd_una の位置）に溜め、窓（相手の窓と輻輳ウィンドウの
//!   小さい方）の分だけ送る。時間切れになったら snd_una から送り直す（go-back-N）。重複 ACK が
//!   3 つ来たときも同じように送り直す
//! - 受信: 順番どおりに来たものだけ `recv_buf` に入れる。飛んで来たものは捨てて、今の位置を ACK
//!   で知らせる（相手が送り直す）
//! - 再送の時間は往復時間から決める（RFC 6298）。輻輳ウィンドウはスロースタートと輻輳回避だけ
//! - やらないこと: SACK, ウィンドウスケール, タイムスタンプ, 遅延 ACK, Nagle, 緊急データ
//!
//! 時刻はミリ秒（呼ぶ側が渡す）。送るセグメントは `out` に積み、IP で包むのは stack の仕事。

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::Ipv4;
use crate::stack::NetError;
use crate::wire::{TCP_ACK, TCP_FIN, TCP_PSH, TCP_RST, TCP_SYN, TcpHeader};

/// 自分の MSS（Ethernet の 1500 - IP 20 - TCP 20）
pub const MSS: u16 = 1460;
/// 相手が MSS を言わなかったとき
const DEFAULT_MSS: u16 = 536;
pub const SEND_BUF: usize = 16 * 1024;
pub const RECV_BUF: usize = 16 * 1024;
const RTO_INITIAL: u64 = 1000;
const RTO_MIN: u64 = 300;
const RTO_MAX: u64 = 30_000;
/// 接続を始めるとき（SYN）の送り直しの回数
const SYN_RETRIES: u32 = 5;
/// データの送り直しの回数（超えたら諦める）
const DATA_RETRIES: u32 = 10;
/// TIME_WAIT の長さ（本来は 2MSL = 数分だが、ポートを早く空けるため短くする）
pub const TIME_WAIT_MS: u64 = 2_000;

/// 通し番号の大小（2^32 で一周するので差の符号で比べる）
fn lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
fn le(a: u32, b: u32) -> bool {
    !lt(b, a)
}
fn gt(a: u32, b: u32) -> bool {
    lt(b, a)
}
fn ge(a: u32, b: u32) -> bool {
    !lt(a, b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

impl TcpState {
    pub fn name(self) -> &'static str {
        match self {
            TcpState::Closed => "CLOSED",
            TcpState::SynSent => "SYN_SENT",
            TcpState::SynReceived => "SYN_RCVD",
            TcpState::Established => "ESTABLISHED",
            TcpState::FinWait1 => "FIN_WAIT_1",
            TcpState::FinWait2 => "FIN_WAIT_2",
            TcpState::CloseWait => "CLOSE_WAIT",
            TcpState::Closing => "CLOSING",
            TcpState::LastAck => "LAST_ACK",
            TcpState::TimeWait => "TIME_WAIT",
        }
    }
}

/// 送る TCP セグメント（宛先は Tcb の remote）
pub struct Segment {
    pub header: TcpHeader,
    pub payload: Vec<u8>,
}

pub struct Tcb {
    pub state: TcpState,
    pub local_port: u16,
    pub remote: (Ipv4, u16),
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    /// これまでに送った一番先の位置（送り直しで snd_nxt を戻しても残る）
    snd_max: u32,
    snd_wnd: u32,
    snd_wl1: u32,
    snd_wl2: u32,
    /// 相手の MSS（1 つのセグメントで送る上限）
    mss: u16,
    irs: u32,
    rcv_nxt: u32,
    /// 最後に知らせた受信の窓
    adv_wnd: u32,
    send_buf: VecDeque<u8>,
    recv_buf: VecDeque<u8>,
    /// こちらの FIN の通し番号（close したら決まる）
    fin_seq: Option<u32>,
    /// 相手の FIN を受け取った（読み終わったら終わり）
    peer_fin: bool,
    rto: u64,
    /// 再送（または窓をのぞく）時刻
    rtx_at: Option<u64>,
    retries: u32,
    srtt: Option<u64>,
    rttvar: u64,
    /// 往復時間を測っているセグメント（その終わりの位置, 送った時刻）
    rtt_probe: Option<(u32, u64)>,
    cwnd: u32,
    ssthresh: u32,
    dup_acks: u32,
    /// 相手の窓が 0 のとき、1 バイトだけ送ってのぞく
    probe: bool,
    ack_now: bool,
    time_wait_until: u64,
    /// 知らせる受信の窓の上限（カードの受信バッファに収まる量にする。`set_window_limit`）
    wnd_limit: u32,
    pub error: Option<NetError>,
}

impl Tcb {
    fn new(local_port: u16, remote: (Ipv4, u16), iss: u32, state: TcpState) -> Tcb {
        Tcb {
            state,
            local_port,
            remote,
            iss,
            snd_una: iss,
            snd_nxt: iss,
            snd_max: iss,
            snd_wnd: 0,
            snd_wl1: 0,
            snd_wl2: 0,
            mss: DEFAULT_MSS,
            irs: 0,
            rcv_nxt: 0,
            adv_wnd: 0,
            send_buf: VecDeque::new(),
            recv_buf: VecDeque::new(),
            fin_seq: None,
            peer_fin: false,
            rto: RTO_INITIAL,
            rtx_at: None,
            retries: 0,
            srtt: None,
            rttvar: 0,
            rtt_probe: None,
            cwnd: 4 * MSS as u32,
            ssthresh: 64 * 1024,
            dup_acks: 0,
            probe: false,
            ack_now: false,
            time_wait_until: 0,
            wnd_limit: RECV_BUF as u32,
            error: None,
        }
    }

    /// 知らせる受信の窓の上限（相手が一度に送ってくる量）。SYN を送る前に決める
    pub fn set_window_limit(&mut self, limit: u32) {
        self.wnd_limit = limit.clamp(MSS as u32, RECV_BUF as u32);
    }

    /// 接続を始める（次の output で SYN を送る）
    pub fn connect(local_port: u16, remote: (Ipv4, u16), iss: u32) -> Tcb {
        Tcb::new(local_port, remote, iss, TcpState::SynSent)
    }

    /// 待ち受けているポートに SYN が来た（次の output で SYN+ACK を送る）
    pub fn from_syn(local_port: u16, remote: (Ipv4, u16), syn: &TcpHeader, iss: u32) -> Tcb {
        let mut t = Tcb::new(local_port, remote, iss, TcpState::SynReceived);
        t.irs = syn.seq;
        t.rcv_nxt = syn.seq.wrapping_add(1);
        t.take_mss(syn);
        t.snd_wnd = syn.window as u32;
        t.snd_wl1 = syn.seq;
        t
    }

    fn take_mss(&mut self, h: &TcpHeader) {
        self.mss = h.mss.unwrap_or(DEFAULT_MSS).clamp(64, MSS);
    }

    /// 送るデータの先頭（send_buf[0]）の通し番号。SYN が届くまでは iss の次
    fn data_start(&self) -> u32 {
        if self.snd_una == self.iss { self.iss.wrapping_add(1) } else { self.snd_una }
    }

    fn rcv_wnd(&self) -> u32 {
        ((RECV_BUF - self.recv_buf.len()) as u32).min(self.wnd_limit).min(u16::MAX as u32)
    }

    fn header(&mut self, seq: u32, flags: u8) -> TcpHeader {
        let window = self.rcv_wnd();
        self.adv_wnd = window;
        TcpHeader {
            src_port: self.local_port,
            dst_port: self.remote.1,
            seq,
            ack: if flags & TCP_ACK != 0 { self.rcv_nxt } else { 0 },
            flags,
            window: window as u16,
            mss: None,
        }
    }

    fn fin_acked(&self) -> bool {
        self.fin_seq.is_some_and(|f| gt(self.snd_una, f))
    }

    fn enter_closed(&mut self) {
        self.state = TcpState::Closed;
        self.rtx_at = None;
    }

    fn enter_time_wait(&mut self, now: u64) {
        self.state = TcpState::TimeWait;
        self.rtx_at = None;
        self.time_wait_until = now + TIME_WAIT_MS;
    }

    /// 届いたセグメントを処理する
    pub fn on_segment(&mut self, h: &TcpHeader, payload: &[u8], now: u64, out: &mut Vec<Segment>) {
        match self.state {
            TcpState::Closed => return,
            TcpState::SynSent => return self.on_segment_syn_sent(h, now, out),
            _ => {}
        }
        let syn = h.flags & TCP_SYN != 0;
        let fin = h.flags & TCP_FIN != 0;
        let rst = h.flags & TCP_RST != 0;

        // こちらの SYN+ACK が届かず、相手が SYN を送り直してきた
        if self.state == TcpState::SynReceived && syn && !rst && h.seq == self.irs {
            self.snd_nxt = self.iss;
            return;
        }

        // 受け取れる範囲（受信の窓）に入っているか
        let seg_len = payload.len() as u32 + syn as u32 + fin as u32;
        let wnd = self.rcv_wnd();
        let rcv_nxt = self.rcv_nxt;
        let in_wnd = |x: u32| ge(x, rcv_nxt) && lt(x, rcv_nxt.wrapping_add(wnd));
        let acceptable = if payload.is_empty() && h.seq == rcv_nxt {
            true // 窓が 0 でも、データの無い制御用のセグメント（ACK・FIN・RST）は受け取る
        } else if seg_len == 0 {
            wnd > 0 && in_wnd(h.seq)
        } else {
            wnd > 0 && (in_wnd(h.seq) || in_wnd(h.seq.wrapping_add(seg_len - 1)))
        };
        if !acceptable {
            if !rst {
                self.ack_now = true; // 古いもの・窓の外のもの: 今の位置を知らせる
            }
            return;
        }

        if rst {
            if self.state != TcpState::SynReceived {
                self.error = Some(NetError::ConnReset);
            }
            self.enter_closed();
            return;
        }
        if syn {
            // つながった後の SYN: 今の位置を知らせるだけ（RFC 5961 の challenge ACK）
            self.ack_now = true;
            return;
        }
        if h.flags & TCP_ACK == 0 {
            return;
        }

        if self.state == TcpState::SynReceived {
            if gt(h.ack, self.snd_una) && le(h.ack, self.snd_max) {
                self.state = TcpState::Established;
                self.snd_wl1 = h.seq;
                self.snd_wl2 = h.ack;
                self.snd_wnd = h.window as u32;
            } else {
                out.push(self.reset_segment(h.ack));
                return;
            }
        }

        // ACK
        if gt(h.ack, self.snd_max) {
            self.ack_now = true; // まだ送っていないところへの ACK
            return;
        }
        if gt(h.ack, self.snd_una) {
            self.advance(h.ack, now);
        } else if h.ack == self.snd_una
            && payload.is_empty()
            && !fin
            && self.snd_max != self.snd_una
            && h.window as u32 == self.snd_wnd
        {
            self.dup_acks += 1;
            if self.dup_acks == 3 {
                // 重複 ACK 3 つ: 落ちたと見て、待たずに送り直す
                let mss = self.mss as u32;
                self.ssthresh = (self.snd_max.wrapping_sub(self.snd_una) / 2).max(2 * mss);
                self.cwnd = self.ssthresh;
                self.snd_nxt = self.snd_una;
                self.rtt_probe = None;
            }
        }
        if lt(self.snd_wl1, h.seq) || (self.snd_wl1 == h.seq && le(self.snd_wl2, h.ack)) {
            self.snd_wnd = h.window as u32;
            self.snd_wl1 = h.seq;
            self.snd_wl2 = h.ack;
            if self.snd_wnd > 0 {
                self.probe = false;
            } else {
                self.retries = 0; // 窓 0 と答えてくる相手は生きている（のぞき続けてよい）
            }
        }
        if self.fin_acked() {
            match self.state {
                TcpState::FinWait1 => self.state = TcpState::FinWait2,
                TcpState::Closing => self.enter_time_wait(now),
                TcpState::LastAck => return self.enter_closed(),
                _ => {}
            }
        }

        // データ
        let mut all_taken = true;
        if !payload.is_empty() {
            if matches!(self.state, TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2) {
                let skip = if lt(h.seq, self.rcv_nxt) { self.rcv_nxt.wrapping_sub(h.seq) as usize } else { 0 };
                if skip <= payload.len() && h.seq.wrapping_add(skip as u32) == self.rcv_nxt {
                    let data = &payload[skip..];
                    let n = data.len().min(RECV_BUF - self.recv_buf.len());
                    self.recv_buf.extend(&data[..n]);
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(n as u32);
                    all_taken = n == data.len();
                } else {
                    all_taken = false; // 順番が飛んだ（落ちたものの再送を待つ）
                }
            }
            self.ack_now = true;
        }

        // FIN（それより前のデータをすべて受け取ったときだけ）
        if fin && all_taken && h.seq.wrapping_add(payload.len() as u32) == self.rcv_nxt {
            self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
            self.peer_fin = true;
            self.ack_now = true;
            match self.state {
                TcpState::Established => self.state = TcpState::CloseWait,
                TcpState::FinWait1 => self.state = TcpState::Closing,
                TcpState::FinWait2 => self.enter_time_wait(now),
                _ => {}
            }
        }
    }

    fn on_segment_syn_sent(&mut self, h: &TcpHeader, now: u64, out: &mut Vec<Segment>) {
        let ack = h.flags & TCP_ACK != 0;
        if ack && (le(h.ack, self.iss) || gt(h.ack, self.snd_max)) {
            if h.flags & TCP_RST == 0 {
                out.push(self.reset_segment(h.ack));
            }
            return;
        }
        if h.flags & TCP_RST != 0 {
            if ack {
                self.error = Some(NetError::ConnRefused);
                self.enter_closed();
            }
            return;
        }
        if h.flags & TCP_SYN == 0 {
            return;
        }
        self.irs = h.seq;
        self.rcv_nxt = h.seq.wrapping_add(1);
        self.take_mss(h);
        self.snd_wnd = h.window as u32;
        self.snd_wl1 = h.seq;
        self.snd_wl2 = h.ack;
        self.ack_now = true;
        if ack {
            self.advance(h.ack, now);
            self.state = TcpState::Established;
        } else {
            // 同時に開いた: SYN+ACK を送り直す
            self.state = TcpState::SynReceived;
            self.snd_nxt = self.iss;
        }
    }

    /// 相手が ack まで受け取った
    fn advance(&mut self, ack: u32, now: u64) {
        let mut n = ack.wrapping_sub(self.snd_una) as usize;
        if self.snd_una == self.iss {
            n -= 1; // SYN の分
        }
        if self.fin_seq.is_some_and(|f| gt(ack, f)) {
            n -= 1; // FIN の分
        }
        let n = n.min(self.send_buf.len());
        self.send_buf.drain(..n);
        self.snd_una = ack;
        if lt(self.snd_nxt, ack) {
            self.snd_nxt = ack;
        }
        if let Some((end, sent)) = self.rtt_probe {
            if ge(ack, end) {
                self.rtt_sample(now.saturating_sub(sent));
                self.rtt_probe = None;
            }
        }
        self.retries = 0;
        self.dup_acks = 0;
        let mss = self.mss as u32;
        self.cwnd = if self.cwnd < self.ssthresh { self.cwnd + mss } else { self.cwnd + (mss * mss / self.cwnd).max(1) };
        self.cwnd = self.cwnd.min(64 * 1024);
        self.rtx_at = if self.snd_una == self.snd_max { None } else { Some(now + self.rto) };
    }

    fn rtt_sample(&mut self, r: u64) {
        let srtt = match self.srtt {
            None => {
                self.rttvar = r / 2;
                r
            }
            Some(s) => {
                self.rttvar = (3 * self.rttvar + s.abs_diff(r)) / 4;
                (7 * s + r) / 8
            }
        };
        self.srtt = Some(srtt);
        self.rto = (srtt + (4 * self.rttvar).max(10)).clamp(RTO_MIN, RTO_MAX);
    }

    /// 時間が経ったときの処理（再送・TIME_WAIT の終わり）と送れるものの送信
    pub fn poll(&mut self, now: u64, out: &mut Vec<Segment>) {
        if self.state == TcpState::TimeWait {
            if now >= self.time_wait_until {
                self.enter_closed();
                return;
            }
        } else if self.rtx_at.is_some_and(|t| now >= t) {
            self.on_timeout(out);
            if self.state == TcpState::Closed {
                return;
            }
        }
        self.output(now, out);
    }

    fn on_timeout(&mut self, out: &mut Vec<Segment>) {
        self.rtx_at = None;
        // 相手の窓が 0 で送れずにいる（1 バイトだけ送ってのぞく）。相手が窓 0 と答える限り諦めない
        let probing = self.snd_wnd == 0 && !self.send_buf.is_empty();
        if !probing {
            let limit = if matches!(self.state, TcpState::SynSent | TcpState::SynReceived) {
                SYN_RETRIES
            } else {
                DATA_RETRIES
            };
            if self.retries >= limit {
                self.error = Some(NetError::TimedOut);
                if self.state != TcpState::SynSent {
                    let seq = self.snd_nxt;
                    let h = self.header(seq, TCP_RST | TCP_ACK);
                    out.push(Segment { header: h, payload: Vec::new() });
                }
                self.enter_closed();
                return;
            }
            self.retries += 1;
        }
        self.rto = (self.rto * 2).min(RTO_MAX);
        let mss = self.mss as u32;
        self.ssthresh = (self.snd_max.wrapping_sub(self.snd_una) / 2).max(2 * mss);
        self.cwnd = mss;
        self.rtt_probe = None;
        self.snd_nxt = self.snd_una;
        self.probe = probing;
    }

    /// 今送れるものを送る（SYN・データ・FIN・ACK）
    pub fn output(&mut self, now: u64, out: &mut Vec<Segment>) {
        match self.state {
            TcpState::Closed => return,
            TcpState::SynSent | TcpState::SynReceived => {
                if self.snd_nxt == self.iss {
                    let flags = if self.state == TcpState::SynSent { TCP_SYN } else { TCP_SYN | TCP_ACK };
                    let mut h = self.header(self.iss, flags);
                    h.mss = Some(MSS);
                    out.push(Segment { header: h, payload: Vec::new() });
                    self.snd_nxt = self.iss.wrapping_add(1);
                    if gt(self.snd_nxt, self.snd_max) {
                        self.snd_max = self.snd_nxt;
                    }
                    self.ack_now = false;
                    if self.rtx_at.is_none() {
                        self.rtx_at = Some(now + self.rto);
                    }
                }
                if self.ack_now && self.state == TcpState::SynReceived {
                    let h = self.header(self.snd_nxt, TCP_ACK);
                    out.push(Segment { header: h, payload: Vec::new() });
                }
                self.ack_now = false;
                return;
            }
            TcpState::TimeWait => {
                if self.ack_now {
                    let h = self.header(self.snd_nxt, TCP_ACK);
                    out.push(Segment { header: h, payload: Vec::new() });
                    self.ack_now = false;
                }
                return;
            }
            _ => {}
        }
        let mut unsent;
        loop {
            let start = self.data_start();
            let off = self.snd_nxt.wrapping_sub(start) as usize;
            let buffered = self.send_buf.len();
            unsent = buffered.saturating_sub(off);
            let in_flight = self.snd_nxt.wrapping_sub(self.snd_una);
            let mut room = self.snd_wnd.min(self.cwnd).saturating_sub(in_flight) as usize;
            if self.probe && unsent > 0 {
                room = room.max(1);
            }
            let n = unsent.min(room).min(self.mss as usize);
            let fin_now = self.fin_seq.is_some_and(|f| self.snd_nxt.wrapping_add(n as u32) == f && off + n == buffered);
            if n == 0 && !fin_now {
                break;
            }
            let mut flags = TCP_ACK;
            if n > 0 {
                flags |= TCP_PSH;
            }
            if fin_now {
                flags |= TCP_FIN;
            }
            let payload: Vec<u8> = self.send_buf.range(off..off + n).copied().collect();
            let seq = self.snd_nxt;
            let h = self.header(seq, flags);
            out.push(Segment { header: h, payload });
            if self.rtt_probe.is_none() && n > 0 && ge(seq, self.snd_max) {
                self.rtt_probe = Some((seq.wrapping_add(n as u32), now));
            }
            self.snd_nxt = seq.wrapping_add(n as u32 + fin_now as u32);
            if gt(self.snd_nxt, self.snd_max) {
                self.snd_max = self.snd_nxt;
            }
            self.ack_now = false;
            self.probe = false;
            if self.rtx_at.is_none() {
                self.rtx_at = Some(now + self.rto);
            }
            if fin_now {
                break;
            }
        }
        // 相手の窓が 0 で送れないデータがある: 時々のぞいて窓が開いたか確かめる
        if self.rtx_at.is_none() && unsent > 0 && self.snd_wnd == 0 {
            self.rtx_at = Some(now + self.rto);
        }
        if self.ack_now {
            let h = self.header(self.snd_nxt, TCP_ACK);
            out.push(Segment { header: h, payload: Vec::new() });
            self.ack_now = false;
        }
    }

    fn reset_segment(&mut self, seq: u32) -> Segment {
        let h = TcpHeader {
            src_port: self.local_port,
            dst_port: self.remote.1,
            seq,
            ack: 0,
            flags: TCP_RST,
            window: 0,
            mss: None,
        };
        Segment { header: h, payload: Vec::new() }
    }

    /// 送るデータを溜める。溜めたバイト数を返す（一杯なら WouldBlock）
    pub fn send(&mut self, data: &[u8]) -> Result<usize, NetError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        match self.state {
            TcpState::Established | TcpState::CloseWait if self.fin_seq.is_none() => {}
            TcpState::SynSent | TcpState::SynReceived => return Err(NetError::WouldBlock),
            _ => return Err(NetError::NotConnected),
        }
        let n = data.len().min(SEND_BUF - self.send_buf.len());
        if n == 0 {
            return Err(NetError::WouldBlock);
        }
        self.send_buf.extend(&data[..n]);
        Ok(n)
    }

    /// 受け取ったデータを読む。0 なら相手が送り終えた（FIN）。まだ何も無ければ WouldBlock
    pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        if !self.recv_buf.is_empty() {
            let n = buf.len().min(self.recv_buf.len());
            for (d, s) in buf.iter_mut().zip(self.recv_buf.drain(..n)) {
                *d = s;
            }
            // 窓が十分に開いたら知らせる（相手が窓 0 で止まっているかもしれない）
            let wnd = self.rcv_wnd();
            if wnd >= self.adv_wnd + self.mss as u32 || (self.adv_wnd < self.mss as u32 && wnd >= self.mss as u32) {
                self.ack_now = true;
            }
            return Ok(n);
        }
        if self.peer_fin {
            return Ok(0);
        }
        if let Some(e) = self.error {
            return Err(e);
        }
        match self.state {
            TcpState::Closed => Ok(0),
            TcpState::SynSent | TcpState::SynReceived | TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {
                Err(NetError::WouldBlock)
            }
            _ => Ok(0),
        }
    }

    /// こちらからは送り終えた（溜まっているデータの後ろに FIN を送る）
    pub fn close(&mut self, out: &mut Vec<Segment>) {
        match self.state {
            TcpState::SynSent => self.enter_closed(),
            TcpState::SynReceived => self.abort(out),
            TcpState::Established | TcpState::CloseWait => {
                self.fin_seq = Some(self.data_start().wrapping_add(self.send_buf.len() as u32));
                self.state =
                    if self.state == TcpState::Established { TcpState::FinWait1 } else { TcpState::LastAck };
            }
            _ => {}
        }
    }

    /// すぐに切る（RST を送る）
    pub fn abort(&mut self, out: &mut Vec<Segment>) {
        if !matches!(self.state, TcpState::Closed | TcpState::SynSent | TcpState::TimeWait) {
            let seq = self.snd_nxt;
            let h = self.header(seq, TCP_RST | TCP_ACK);
            out.push(Segment { header: h, payload: Vec::new() });
        }
        self.enter_closed();
    }

    /// 読めるもの（データ・終わり・エラー）がある
    pub fn readable(&self) -> bool {
        !self.recv_buf.is_empty() || self.peer_fin || self.error.is_some() || self.state == TcpState::Closed
    }

    /// 書ける（溜める場所がある）か、もう書けないことが分かっている
    pub fn writable(&self) -> bool {
        match self.state {
            TcpState::Established | TcpState::CloseWait => self.send_buf.len() < SEND_BUF || self.fin_seq.is_some(),
            TcpState::SynSent | TcpState::SynReceived => false,
            _ => true,
        }
    }

    /// 接続の手順（SYN のやり取り）が終わった（成功でも失敗でも）
    pub fn connect_done(&self) -> bool {
        !matches!(self.state, TcpState::SynSent | TcpState::SynReceived)
    }

    /// 送ったデータがすべて届いた
    pub fn all_acked(&self) -> bool {
        self.send_buf.is_empty() && self.snd_una == self.snd_max
    }

    /// 次に時間切れになる時刻
    pub fn next_deadline(&self) -> Option<u64> {
        if self.state == TcpState::TimeWait { Some(self.time_wait_until) } else { self.rtx_at }
    }

    /// 受け取って、まだ読まれていないバイト数
    pub fn recv_queued(&self) -> usize {
        self.recv_buf.len()
    }

    /// 送るために溜めている（まだ届いていない）バイト数
    pub fn send_queued(&self) -> usize {
        self.send_buf.len()
    }
}
