// author: kodeholic (powered by Claude)
//
// lctrace — mini-livechat 실시간 시그널링 관찰 CLI
//
// 사용법:
//   lctrace [--host HOST] [--port PORT] [--filter OP] [CHANNEL_ID]
//
// 예시:
//   lctrace                          # 전체 이벤트 스트림
//   lctrace CH_0001                  # CH_0001 채널만
//   lctrace --filter floor           # Floor 이벤트만 (전체 채널)
//   lctrace CH_0001 --filter floor   # CH_0001 + Floor 이벤트만
//   lctrace --host 192.168.1.10 --port 8080 CH_0001

use clap::Parser;
use colored::Colorize;
use reqwest::blocking::Client;
use std::io::{BufRead, BufReader};

use serde::Deserialize;

// ----------------------------------------------------------------------------
// [CLI 인자]
// ----------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name    = "lctrace",
    about   = "mini-livechat 실시간 시그널링 이벤트 스트림 관찰",
    version,
)]
struct Cli {
    /// 서버 호스트
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 서버 포트
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// 이벤트 필터 키워드 (예: floor, channel, identify)
    /// 대소문자 무관, op_name 부분 일치
    #[arg(long, short = 'f')]
    filter: Option<String>,

    /// 관찰할 채널 ID (생략 시 전체)
    channel_id: Option<String>,
}

// ----------------------------------------------------------------------------
// [TraceEvent 역직렬화] — src/trace.rs와 동일 구조
// ----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TraceEvent {
    ts:         u64,
    dir:        String,        // "in" | "out" | "sys"
    channel_id: Option<String>,
    user_id:    Option<String>,
    op:         u8,
    op_name:    String,
    summary:    String,
}

// ----------------------------------------------------------------------------
// [메인]
// ----------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let url = match &cli.channel_id {
        Some(ch) => format!("http://{}:{}/trace/{}", cli.host, cli.port, ch),
        None     => format!("http://{}:{}/trace",   cli.host, cli.port),
    };

    let filter = cli.filter.as_ref().map(|s| s.to_lowercase());

    // 헤더 출력
    println!("{}", "─".repeat(90).dimmed());
    println!(
        "  {} {}  {}  {}",
        "lctrace".bold().cyan(),
        "▶".green(),
        url.dimmed(),
        filter.as_deref()
            .map(|f| format!("[filter: {}]", f).yellow().to_string())
            .unwrap_or_default(),
    );
    println!("{}", "─".repeat(90).dimmed());
    println!(
        "  {:<12} {:<5} {:<6} {:<20} {:<18} {}",
        "TIME".dimmed(),
        "OP".dimmed(),
        "DIR".dimmed(),
        "OP_NAME".dimmed(),
        "USER".dimmed(),
        "SUMMARY".dimmed(),
    );
    println!("{}", "─".repeat(90).dimmed());

    // SSE 스트림 연결 (blocking, chunked read)
    let client = Client::builder()
        .timeout(None) // 스트림이라 타임아웃 없음
        .build()
        .expect("reqwest client 생성 실패");

    let resp = match client
        .get(&url)
        .header("Accept", "text/event-stream")
        .send()
    {
        Ok(r)  => r,
        Err(e) => {
            eprintln!("{} 서버 연결 실패: {}", "✗".red(), e);
            eprintln!("  서버가 실행 중인지 확인하세요: {}", url.dimmed());
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!("{} HTTP {}", "✗".red(), resp.status());
        std::process::exit(1);
    }

    let reader = BufReader::new(resp);
    let mut event_count: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l)  => l,
            Err(e) => {
                eprintln!("{} 스트림 읽기 실패: {}", "✗".red(), e);
                break;
            }
        };

        // SSE 포맷: "data: {JSON}" 또는 ": keep-alive" 또는 빈 줄
        if line.starts_with(": ") {
            // keep-alive 주석 — 무시
            continue;
        }

        let json_str = if let Some(rest) = line.strip_prefix("data: ") {
            rest
        } else {
            continue;
        };

        let event: TraceEvent = match serde_json::from_str(json_str) {
            Ok(e)  => e,
            Err(e) => {
                eprintln!("{} JSON 파싱 실패: {} ({})", "⚠".yellow(), e, json_str);
                continue;
            }
        };

        // 클라이언트 측 op_name 필터 (서버 SSE 채널 필터와 별도)
        if let Some(ref f) = filter {
            if !event.op_name.to_lowercase().contains(f.as_str()) {
                continue;
            }
        }

        print_event(&event);
        event_count += 1;
        let _ = event_count; // 나중에 통계 출력용
    }

    println!("{}", "─".repeat(90).dimmed());
    println!("  스트림 종료 (총 {} 이벤트)", event_count);
}

// ----------------------------------------------------------------------------
// [이벤트 출력]
// ----------------------------------------------------------------------------

fn print_event(e: &TraceEvent) {
    let time_str = format_ts(e.ts);

    // 방향 컬러링
    let dir_str = match e.dir.as_str() {
        "in"  => "↓ C→S".bright_blue().to_string(),
        "out" => "↑ S→C".bright_green().to_string(),
        "sys" => "· SYS".bright_yellow().to_string(),
        other => other.dimmed().to_string(),
    };

    // op_name 컬러링 (Floor 이벤트 강조)
    let op_name_str = colorize_op_name(&e.op_name);

    let user_str = e.user_id.as_deref().unwrap_or("-");
    let ch_str   = e.channel_id.as_deref().unwrap_or("—");

    println!(
        "  {} {:>3} {} {:<22} {:<18} {} {}",
        time_str.dimmed(),
        format!("{}", e.op).dimmed(),
        dir_str,
        op_name_str,
        // user 18자 truncate
        truncate(user_str, 18).bright_white().to_string(),
        ch_str.dimmed(),
        e.summary.dimmed(),
    );
}

fn colorize_op_name(name: &str) -> String {
    let upper = name.to_uppercase();
    if upper.contains("GRANTED") {
        name.bright_green().bold().to_string()
    } else if upper.contains("REVOKE") || upper.contains("DENY") {
        name.bright_red().bold().to_string()
    } else if upper.contains("FLOOR") {
        name.bright_yellow().to_string()
    } else if upper.contains("JOIN") || upper.contains("LEAVE") {
        name.bright_cyan().to_string()
    } else if upper.contains("IDENTIFY") {
        name.bright_magenta().to_string()
    } else {
        name.normal().to_string()
    }
}

/// Unix millis → "HH:MM:SS.mmm" (로컬 시각 근사 — UTC+9 오프셋 없이 UTC 표시)
fn format_ts(ts_ms: u64) -> String {
    let secs   = ts_ms / 1000;
    let millis = ts_ms % 1000;

    // 간단히 Unix epoch 기준 HH:MM:SS 계산
    let total_secs_today = secs % 86400;
    let hh = total_secs_today / 3600;
    let mm = (total_secs_today % 3600) / 60;
    let ss = total_secs_today % 60;

    format!("{:02}:{:02}:{:02}.{:03}", hh, mm, ss, millis)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        format!("{:<width$}", s, width = max)
    } else {
        format!("{}…", &s[..max - 1])
    }
}
