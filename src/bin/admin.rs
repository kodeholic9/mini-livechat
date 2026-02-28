// author: kodeholic (powered by Claude)
//
// lcadmin — mini-livechat 운영 관리 CLI
//
// 사용법:
//   lcadmin [--host HOST] [--port PORT] <COMMAND>
//
// 조회 명령
//   lcadmin status                    서버 상태 요약 (uptime, 연결 수, Floor 활성)
//   lcadmin users                     User 전체 테이블
//   lcadmin users <user_id>           User 상세
//   lcadmin channels                  Channel 전체 테이블 (Floor 상태 포함)
//   lcadmin channels <channel_id>     Channel 상세 (대기열, peer 목록)
//   lcadmin peers                     Endpoint 전체 테이블
//   lcadmin peers <ufrag>             Endpoint 상세
//
// 조작 명령
//   lcadmin floor-revoke <channel_id> Floor 강제 revoke

use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::Deserialize;
use tabled::{Table, Tabled};

// ----------------------------------------------------------------------------
// [CLI 정의]
// ----------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name    = "lcadmin",
    about   = "mini-livechat 운영 관리 CLI",
    version,
)]
struct Cli {
    /// 서버 호스트
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 서버 포트 (WS/HTTP 공용)
    #[arg(long, default_value_t = 8080)]
    port: u16,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 서버 상태 요약 (uptime, 연결 수, Floor 활성 채널)
    Status,

    /// User 목록 또는 상세
    Users {
        /// user_id 지정 시 상세 보기
        user_id: Option<String>,
    },

    /// Channel 목록 또는 상세
    Channels {
        /// channel_id 지정 시 상세 보기
        channel_id: Option<String>,
    },

    /// Endpoint(Peer) 목록 또는 상세
    Peers {
        /// ufrag 지정 시 상세 보기
        ufrag: Option<String>,
    },

    /// Floor 강제 revoke
    FloorRevoke {
        /// 대상 channel_id
        channel_id: String,
    },
}

// ----------------------------------------------------------------------------
// [응답 타입] — http.rs 와 대응
// ----------------------------------------------------------------------------

#[derive(Deserialize)]
struct ServerStatus {
    uptime_secs:   u64,
    user_count:    usize,
    channel_count: usize,
    peer_count:    usize,
    floor_active:  usize,
}

#[derive(Deserialize, Tabled)]
struct AdminUserSummary {
    #[tabled(rename = "USER ID")]
    user_id:      String,
    #[tabled(rename = "PRI")]
    priority:     u8,
    #[tabled(rename = "IDLE(s)")]
    idle_secs:    u64,
}

#[derive(Deserialize)]
struct AdminUserDetail {
    user_id:      String,
    priority:     u8,
    last_seen_ms: u64,
    idle_secs:    u64,
    channels:     Vec<String>,
}

#[derive(Deserialize, Tabled)]
struct AdminChannelSummary {
    #[tabled(rename = "CHANNEL ID")]
    channel_id:   String,
    #[tabled(rename = "FREQ")]
    freq:         String,
    #[tabled(rename = "NAME")]
    name:         String,
    #[tabled(rename = "MEMBERS")]
    member_count: usize,
    #[tabled(rename = "CAP")]
    capacity:     usize,
    #[tabled(rename = "FLOOR")]
    floor_state:  String,
    #[tabled(skip)]
    floor_holder: Option<String>,
    #[tabled(rename = "QUEUE")]
    queue_len:    usize,
}

#[derive(Deserialize)]
struct AdminChannelDetail {
    channel_id:       String,
    freq:             String,
    name:             String,
    capacity:         usize,
    created_at:       u64,
    members:          Vec<String>,
    floor_state:      String,
    floor_holder:     Option<String>,
    floor_taken_secs: Option<u64>,
    floor_priority:   u8,
    queue_len:        usize,
    queue:            Vec<AdminQueueEntry>,
    peers:            Vec<AdminPeerSummary>,
}

#[derive(Deserialize, Tabled)]
struct AdminQueueEntry {
    #[tabled(rename = "USER ID")]
    user_id:   String,
    #[tabled(rename = "PRI")]
    priority:  u8,
    #[tabled(rename = "WAIT(s)")]
    wait_secs: u64,
}

#[derive(Deserialize, Tabled)]
struct AdminPeerSummary {
    #[tabled(rename = "UFRAG")]
    ufrag:      String,
    #[tabled(rename = "USER ID")]
    user_id:    String,
    #[tabled(rename = "CHANNEL")]
    channel_id: String,
    #[tabled(rename = "ADDRESS")]
    #[serde(deserialize_with = "deser_opt_string")]
    address:    String,
    #[tabled(rename = "IDLE(s)")]
    idle_secs:  u64,
    #[tabled(rename = "SRTP")]
    srtp_ready: bool,
}

#[derive(Deserialize)]
struct AdminPeerDetail {
    ufrag:      String,
    user_id:    String,
    channel_id: String,
    address:    Option<String>,
    last_seen:  u64,
    idle_secs:  u64,
    srtp_ready: bool,
    tracks:     Vec<AdminTrack>,
}

#[derive(Deserialize, Tabled)]
struct AdminTrack {
    #[tabled(rename = "SSRC")]
    ssrc: u32,
    #[tabled(rename = "KIND")]
    kind: String,
}

// ----------------------------------------------------------------------------
// [main]
// ----------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();
    let base = format!("http://{}:{}", cli.host, cli.port);

    let result = match &cli.command {
        Command::Status                        => cmd_status(&base),
        Command::Users { user_id: None }       => cmd_users(&base),
        Command::Users { user_id: Some(uid) }  => cmd_user_detail(&base, uid),
        Command::Channels { channel_id: None } => cmd_channels(&base),
        Command::Channels { channel_id: Some(cid) } => cmd_channel_detail(&base, cid),
        Command::Peers { ufrag: None }         => cmd_peers(&base),
        Command::Peers { ufrag: Some(uf) }     => cmd_peer_detail(&base, uf),
        Command::FloorRevoke { channel_id }    => cmd_floor_revoke(&base, channel_id),
    };

    if let Err(e) = result {
        eprintln!("{} {}", "ERROR:".red().bold(), e);
        std::process::exit(1);
    }
}

// ----------------------------------------------------------------------------
// [커맨드 구현]
// ----------------------------------------------------------------------------

fn cmd_status(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    let s: ServerStatus = get_json(&format!("{}/admin/status", base))?;

    let hours   = s.uptime_secs / 3600;
    let minutes = (s.uptime_secs % 3600) / 60;
    let secs    = s.uptime_secs % 60;

    println!();
    println!("{}", "  mini-livechat Server Status".bold().cyan());
    println!("  {}", "─".repeat(36).dimmed());
    println!("  {:16} {}",
        "Uptime:".bold(),
        format!("{}h {}m {}s", hours, minutes, secs).green()
    );
    println!("  {:16} {}", "Users:".bold(),    s.user_count.to_string().yellow());
    println!("  {:16} {}", "Channels:".bold(), s.channel_count.to_string().yellow());
    println!("  {:16} {}", "Peers:".bold(),    s.peer_count.to_string().yellow());
    println!("  {:16} {}",
        "Floor Active:".bold(),
        if s.floor_active > 0 {
            s.floor_active.to_string().red().bold().to_string()
        } else {
            s.floor_active.to_string().dimmed().to_string()
        }
    );
    println!();
    Ok(())
}

fn cmd_users(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut users: Vec<AdminUserSummary> = get_json(&format!("{}/admin/users", base))?;

    if users.is_empty() {
        println!("{}", "  접속 중인 User 없음".dimmed());
        return Ok(());
    }

    // idle 시간에 따라 컬러 적용
    for u in &mut users {
        if u.idle_secs > 60 {
            u.user_id = u.user_id.red().to_string();
        }
    }

    println!();
    println!("{}", Table::new(&users).to_string());
    println!("  {} user(s)", users.len());
    println!();
    Ok(())
}

fn cmd_user_detail(base: &str, user_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let u: AdminUserDetail = get_json(&format!("{}/admin/users/{}", base, user_id))?;

    println!();
    println!("{}", format!("  User: {}", u.user_id).bold().cyan());
    println!("  {}", "─".repeat(36).dimmed());
    println!("  {:16} {}", "Priority:".bold(),  u.priority);
    println!("  {:16} {}s", "Idle:".bold(),      u.idle_secs);
    println!("  {:16} {}", "Last Seen:".bold(),  format_ts(u.last_seen_ms));
    println!("  {:16} {}",
        "Channels:".bold(),
        if u.channels.is_empty() {
            "(없음)".dimmed().to_string()
        } else {
            u.channels.join(", ").yellow().to_string()
        }
    );
    println!();
    Ok(())
}

fn cmd_channels(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    let channels: Vec<AdminChannelSummary> = get_json(&format!("{}/admin/channels", base))?;

    if channels.is_empty() {
        println!("{}", "  채널 없음".dimmed());
        return Ok(());
    }

    // Floor Taken 채널 강조
    let display: Vec<AdminChannelSummaryDisplay> = channels.iter().map(|ch| {
        AdminChannelSummaryDisplay {
            channel_id:   ch.channel_id.clone(),
            freq:         ch.freq.clone(),
            name:         ch.name.clone(),
            member_count: ch.member_count,
            capacity:     ch.capacity,
            floor_state:  if ch.floor_state == "taken" {
                "● TAKEN".red().bold().to_string()
            } else {
                "○ idle".dimmed().to_string()
            },
            floor_holder: ch.floor_holder.clone().unwrap_or_else(|| "-".to_string()),
            queue_len:    ch.queue_len,
        }
    }).collect();

    println!();
    println!("{}", Table::new(&display).to_string());
    println!("  {} channel(s)", channels.len());
    println!();
    Ok(())
}

// 컬러 렌더링용 표시 타입 (Tabled 적용)
#[derive(Tabled)]
struct AdminChannelSummaryDisplay {
    #[tabled(rename = "CHANNEL ID")]
    channel_id:   String,
    #[tabled(rename = "FREQ")]
    freq:         String,
    #[tabled(rename = "NAME")]
    name:         String,
    #[tabled(rename = "MEMBERS")]
    member_count: usize,
    #[tabled(rename = "CAP")]
    capacity:     usize,
    #[tabled(rename = "FLOOR")]
    floor_state:  String,
    #[tabled(rename = "HOLDER")]
    floor_holder: String,
    #[tabled(rename = "Q")]
    queue_len:    usize,
}

fn cmd_channel_detail(base: &str, channel_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ch: AdminChannelDetail = get_json(&format!("{}/admin/channels/{}", base, channel_id))?;

    println!();
    println!("{}", format!("  Channel: {} [{}] {}", ch.channel_id, ch.freq, ch.name).bold().cyan());
    println!("  {}", "─".repeat(48).dimmed());
    println!("  {:18} {}", "Capacity:".bold(),    format!("{}/{}", ch.members.len(), ch.capacity));
    println!("  {:18} {}", "Created:".bold(),      format_ts(ch.created_at));

    // Floor 상태
    let floor_line = if ch.floor_state == "taken" {
        format!("{} (holder: {}, {}s 경과, priority: {})",
            "● TAKEN".red().bold(),
            ch.floor_holder.as_deref().unwrap_or("-").yellow(),
            ch.floor_taken_secs.unwrap_or(0),
            ch.floor_priority,
        )
    } else {
        "○ idle".dimmed().to_string()
    };
    println!("  {:18} {}", "Floor:".bold(), floor_line);

    // 멤버 목록
    println!();
    println!("{}", "  Members".bold());
    if ch.members.is_empty() {
        println!("    {}", "(없음)".dimmed());
    } else {
        for m in &ch.members {
            println!("    · {}", m.yellow());
        }
    }

    // 대기열
    if !ch.queue.is_empty() {
        println!();
        println!("{} ({})", "  Floor Queue".bold(), ch.queue_len);
        println!("{}", Table::new(&ch.queue).to_string()
            .lines()
            .map(|l| format!("  {}", l))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    // Peer 목록
    if !ch.peers.is_empty() {
        println!();
        println!("{}", "  Peers".bold());
        println!("{}", Table::new(&ch.peers).to_string()
            .lines()
            .map(|l| format!("  {}", l))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    println!();
    Ok(())
}

fn cmd_peers(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    let peers: Vec<AdminPeerSummary> = get_json(&format!("{}/admin/peers", base))?;

    if peers.is_empty() {
        println!("{}", "  접속 중인 Peer 없음".dimmed());
        return Ok(());
    }

    println!();
    println!("{}", Table::new(&peers).to_string());
    println!("  {} peer(s)", peers.len());
    println!();
    Ok(())
}

fn cmd_peer_detail(base: &str, ufrag: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ep: AdminPeerDetail = get_json(&format!("{}/admin/peers/{}", base, ufrag))?;

    println!();
    println!("{}", format!("  Peer: {}", ep.ufrag).bold().cyan());
    println!("  {}", "─".repeat(36).dimmed());
    println!("  {:16} {}", "User ID:".bold(),    ep.user_id.yellow());
    println!("  {:16} {}", "Channel:".bold(),    ep.channel_id);
    println!("  {:16} {}", "Address:".bold(),    ep.address.as_deref().unwrap_or("(latching 전)").dimmed());
    println!("  {:16} {}s", "Idle:".bold(),      ep.idle_secs);
    println!("  {:16} {}", "Last Seen:".bold(),  format_ts(ep.last_seen));
    println!("  {:16} {}",
        "SRTP:".bold(),
        if ep.srtp_ready { "✓ ready".green().to_string() } else { "✗ not ready".red().to_string() }
    );

    if !ep.tracks.is_empty() {
        println!();
        println!("{}", "  Tracks".bold());
        println!("{}", Table::new(&ep.tracks).to_string()
            .lines()
            .map(|l| format!("  {}", l))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    println!();
    Ok(())
}

fn cmd_floor_revoke(base: &str, channel_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::new();
    let url    = format!("{}/admin/floor-revoke/{}", base, channel_id);
    let resp   = client.post(&url).send()?;

    let status = resp.status();
    let body: serde_json::Value = resp.json()?;

    if status.is_success() {
        let revoked = body["revoked_from"].as_str().unwrap_or("-");
        println!();
        println!("  {} channel={} revoked_from={}",
            "Floor Revoke OK".green().bold(),
            channel_id.yellow(),
            revoked.cyan(),
        );
        println!();
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error");
        return Err(format!("[{}] {}", status, msg).into());
    }

    Ok(())
}

// ----------------------------------------------------------------------------
// [공통 유틸]
// ----------------------------------------------------------------------------

/// Option<String> JSON → String ("-" 폴백)
fn deser_opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v: Option<String> = serde::Deserialize::deserialize(d)?;
    Ok(v.unwrap_or_else(|| "-".to_string()))
}

/// GET 요청 + JSON 역직렬화
fn get_json<T: for<'de> serde::Deserialize<'de>>(url: &str) -> Result<T, Box<dyn std::error::Error>> {
    let resp = reqwest::blocking::get(url)?;
    let status = resp.status();
    if !status.is_success() {
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let msg = body["error"].as_str().unwrap_or("unknown error");
        return Err(format!("[{}] {}", status, msg).into());
    }
    Ok(resp.json()?)
}

/// Unix millis → "YYYY-MM-DD HH:MM:SS" (로컬 시간 근사, UTC 기준)
fn format_ts(ms: u64) -> String {
    if ms == 0 { return "-".to_string(); }
    let secs = ms / 1000;
    // chrono 없이 간단 포맷 (UTC 기준)
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let d = secs / 86400;
    // 1970-01-01 기준 대략적 날짜 (운영 로그 참고용)
    format!("day+{} {:02}:{:02}:{:02} UTC", d, h, m, s)
}
