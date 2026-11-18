//! clone-chzzk 오토스케일러(과제 5, ADR 0006).
//!
//! Docker Compose로 띄운 서버(`app`) 인스턴스들의 CPU 사용률을 주기적으로 재고, 목표 사용률에 맞춰
//! 최소·최대 범위 안에서 인스턴스 수를 바꾼다.
//! - 늘리기: `docker compose up -d --no-deps --no-recreate --scale app=N app`
//! - 줄이기: 각 인스턴스의 `/healthz`를 보고 라이브 송출을 받고 있지 않은 인스턴스만 골라
//!   `docker stop`(SIGTERM, 정상 종료) 뒤 `docker rm` 한다.
//!
//! 판단 규칙은 [`policy`]에 있다.

mod policy;

use std::{
    env,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    process::Command,
    time::{Duration, Instant},
};

use policy::{Instance, Policy, Stabilizer};

struct Config {
    policy: Policy,
    interval: Duration,
    scale_down_stabilization: Duration,
    project: String,
    service: String,
    compose_file: String,
    app_port: u16,
    stop_timeout_secs: u64,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let min = number("AUTOSCALER_MIN", 1)?;
        let max = number("AUTOSCALER_MAX", 5)?;
        if min == 0 || max < min {
            return Err(format!(
                "AUTOSCALER_MIN({min})은 1 이상, AUTOSCALER_MAX({max})는 MIN 이상이어야 합니다"
            ));
        }
        Ok(Self {
            policy: Policy {
                min,
                max,
                target_cpu: number("AUTOSCALER_TARGET_CPU", 50.0)?,
                tolerance: number("AUTOSCALER_TOLERANCE", 0.1)?,
            },
            interval: Duration::from_secs(number("AUTOSCALER_INTERVAL_SECS", 10)?),
            scale_down_stabilization: Duration::from_secs(number(
                "AUTOSCALER_SCALE_DOWN_STABILIZATION_SECS",
                60,
            )?),
            project: text("AUTOSCALER_PROJECT", "chzzk"),
            service: text("AUTOSCALER_SERVICE", "app"),
            compose_file: text("AUTOSCALER_COMPOSE_FILE", "/project/compose.yaml"),
            app_port: number("AUTOSCALER_APP_PORT", 3000)?,
            stop_timeout_secs: number("AUTOSCALER_STOP_TIMEOUT_SECS", 30)?,
        })
    }
}

fn text(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn number<T: std::str::FromStr>(key: &str, default: T) -> Result<T, String> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .trim()
            .parse()
            .map_err(|_| format!("{key}는 숫자여야 합니다: {value}")),
        _ => Ok(default),
    }
}

fn main() {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("[autoscaler] 설정 오류: {err}");
            std::process::exit(2);
        }
    };
    log(&format!(
        "시작: {}/{} 인스턴스 {}~{}개, 목표 CPU {}%(허용 오차 {}%), {}초마다 확인, 줄이기 안정화 {}초",
        config.project,
        config.service,
        config.policy.min,
        config.policy.max,
        config.policy.target_cpu,
        config.policy.tolerance * 100.0,
        config.interval.as_secs(),
        config.scale_down_stabilization.as_secs(),
    ));
    let mut stabilizer = Stabilizer::new(config.scale_down_stabilization);
    loop {
        if let Err(err) = tick(&config, &mut stabilizer) {
            log(&format!("이번 확인을 건너뜁니다: {err}"));
        }
        std::thread::sleep(config.interval);
    }
}

/// 한 번 재고, 필요하면 인스턴스 수를 바꾼다.
fn tick(config: &Config, stabilizer: &mut Stabilizer) -> Result<(), String> {
    let containers = list_containers(config)?;
    let current = containers.len();
    let cpu = if containers.is_empty() {
        Vec::new()
    } else {
        cpu_usage(&containers)?
    };
    let average = if cpu.is_empty() {
        0.0
    } else {
        cpu.iter().sum::<f64>() / cpu.len() as f64
    };
    let desired = config.policy.desired_replicas(current, average);
    let target = stabilizer.recommend(Instant::now(), current, desired);
    let per_instance = cpu
        .iter()
        .map(|value| format!("{value:.0}%"))
        .collect::<Vec<_>>()
        .join(" ");
    log(&format!(
        "인스턴스 {current}개 CPU [{per_instance}] 평균 {average:.1}% (목표 {}%) → 원하는 수 {desired}, 맞출 수 {target}",
        config.policy.target_cpu
    ));

    if target > current {
        log(&format!("스케일 아웃: {current} → {target}"));
        scale_out(config, target)?;
    } else if target < current {
        scale_in(config, &containers, current - target)?;
    }
    Ok(())
}

/// 실행 중인 app 컨테이너 이름들.
fn list_containers(config: &Config) -> Result<Vec<String>, String> {
    let output = docker(&[
        "ps",
        "--filter",
        &format!("label=com.docker.compose.project={}", config.project),
        "--filter",
        &format!("label=com.docker.compose.service={}", config.service),
        "--filter",
        "status=running",
        "--format",
        "{{.Names}}",
    ])?;
    let mut names: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    names.sort();
    Ok(names)
}

/// 컨테이너마다 CPU 사용률(%). 코어 1개를 다 쓰면 100%다.
fn cpu_usage(containers: &[String]) -> Result<Vec<f64>, String> {
    let mut args = vec!["stats", "--no-stream", "--format", "{{.CPUPerc}}"];
    args.extend(containers.iter().map(String::as_str));
    let output = docker(&args)?;
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.trim_end_matches('%')
                .parse::<f64>()
                .map_err(|_| format!("docker stats의 CPU 값을 읽을 수 없습니다: {line}"))
        })
        .collect()
}

fn scale_out(config: &Config, target: usize) -> Result<(), String> {
    let scale = format!("{}={target}", config.service);
    compose(
        config,
        &[
            "up",
            "-d",
            "--no-deps",
            "--no-recreate",
            "--no-build",
            "--scale",
            &scale,
            &config.service,
        ],
    )?;
    Ok(())
}

/// 송출을 받고 있지 않은 인스턴스를 골라 정상 종료시킨다.
fn scale_in(config: &Config, containers: &[String], count: usize) -> Result<(), String> {
    let instances: Vec<Instance> = containers
        .iter()
        .map(|name| health(name, config.app_port))
        .collect();
    let removals = policy::pick_removals(&instances, count);
    if removals.len() < count {
        log(&format!(
            "줄일 수 있는 인스턴스가 {}개뿐입니다(송출 중이거나 응답 없는 인스턴스는 남깁니다)",
            removals.len()
        ));
    }
    let mut remaining = containers.len();
    for name in removals {
        log(&format!(
            "스케일 인: {name} 정상 종료 ({remaining}개 → {}개)",
            remaining - 1
        ));
        remaining -= 1;
        // SIGTERM을 받은 서버는 헬스 체크를 실패로 바꾸고 처리 중인 요청을 마친 뒤 내려간다.
        docker(&[
            "stop",
            "--timeout",
            &config.stop_timeout_secs.to_string(),
            &name,
        ])?;
        docker(&["rm", &name])?;
    }
    Ok(())
}

/// 인스턴스의 `/healthz`. 답하지 않으면 상태를 모르는 것으로 둔다.
fn health(name: &str, port: u16) -> Instance {
    let unknown = Instance {
        name: name.to_owned(),
        live_ingests: None,
        shutting_down: false,
    };
    let Ok(body) = http_get(name, port, "/healthz") else {
        return unknown;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return unknown;
    };
    Instance {
        name: name.to_owned(),
        live_ingests: json["live_ingests"].as_u64(),
        shutting_down: json["shutting_down"].as_bool().unwrap_or(true),
    }
}

/// 같은 Docker 네트워크의 컨테이너에 HTTP GET을 보내 본문을 돌려준다.
fn http_get(host: &str, port: u16, path: &str) -> std::io::Result<String> {
    let timeout = Duration::from_secs(2);
    let address = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other("주소를 찾을 수 없습니다"))?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .ok_or_else(|| std::io::Error::other("HTTP 응답 형식이 아닙니다"))
}

fn compose(config: &Config, args: &[&str]) -> Result<String, String> {
    let mut full = vec![
        "compose",
        "--project-name",
        &config.project,
        "--file",
        &config.compose_file,
    ];
    full.extend_from_slice(args);
    docker(&full)
}

fn docker(args: &[&str]) -> Result<String, String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|err| format!("docker를 실행할 수 없습니다: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "docker {} 실패: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn log(message: &str) {
    // UTC 시:분:초. 날짜가 필요하면 `docker compose logs -t`로 본다.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
        % 86_400;
    println!(
        "[autoscaler {:02}:{:02}:{:02}Z] {message}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
}
