use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Context;

use crate::logger;

#[derive(Clone, Default)]
pub struct LcuOverrides {
    lockfile: Option<PathBuf>,
    lol_dir: Option<PathBuf>,
}

impl LcuOverrides {
    pub fn from_args(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut overrides = Self::default();
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--lockfile" => {
                    let value = args.next().context("missing value for --lockfile")?;
                    overrides.lockfile = Some(PathBuf::from(value));
                }
                "--lol-dir" => {
                    let value = args.next().context("missing value for --lol-dir")?;
                    overrides.lol_dir = Some(PathBuf::from(value));
                }
                "--help" | "-h" | "/?" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => anyhow::bail!("unknown argument: {arg} (use --help)"),
            }
        }

        Ok(overrides)
    }
}

#[derive(Clone)]
struct LcuAuth {
    protocol: String,
    port: u16,
    password: String,
}

pub fn run(stop: Arc<AtomicBool>, overrides: LcuOverrides) {
    logger::info("lcu worker start");

    let client = match build_client() {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!("build HTTP client failed: {err:?}"));
            return;
        }
    };

    let candidates = candidate_lockfile_paths(&overrides);
    if candidates.is_empty() {
        logger::warn("no candidate lockfile paths (use --lol-dir or --lockfile)");
    } else {
        logger::info(&format!("candidate lockfiles ({}):", candidates.len()));
        for path in candidates {
            logger::info(&format!("  {}", path.display()));
        }
    }

    let running = running_league_lockfile_paths();
    if running.is_empty() {
        logger::info("running League client lockfiles: none");
    } else {
        logger::info(&format!(
            "running League client lockfiles ({}):",
            running.len()
        ));
        for path in running {
            logger::info(&format!("  {}", path.display()));
        }
    }

    let mut accepted_this_ready_check = false;
    let mut connection: Option<(LcuAuth, PathBuf)> = None;
    let mut last_phase: Option<String> = None;

    while !stop.load(Ordering::Relaxed) {
        if connection.is_none() {
            connection = discover_connection(&client, &overrides);
            accepted_this_ready_check = false;
            last_phase = None;

            if connection.is_none() {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }

        let Some((auth, lockfile_path)) = connection.as_ref() else {
            continue;
        };

        let phase = match get_gameflow_phase(&client, auth) {
            Ok(phase) => phase,
            Err(err) => {
                logger::warn(&format!(
                    "LCU request failed (lockfile={}): {err:?}",
                    lockfile_path.display()
                ));
                connection = None;
                accepted_this_ready_check = false;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        if last_phase.as_deref() != Some(phase.as_str()) {
            logger::info(&format!("gameflow phase: {phase}"));
            last_phase = Some(phase.clone());
        }

        if phase == "ReadyCheck" {
            if !accepted_this_ready_check {
                logger::info("ready-check: attempting accept");
                match accept_ready_check(&client, auth) {
                    Ok(()) => {
                        logger::info("ready-check: accepted");
                        accepted_this_ready_check = true;
                    }
                    Err(err) => {
                        logger::warn(&format!("ready-check accept failed: {err:?}"));
                    }
                }
            }
        } else {
            accepted_this_ready_check = false;
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    logger::info("lcu worker stop");
}

fn discover_connection(
    client: &reqwest::blocking::Client,
    overrides: &LcuOverrides,
) -> Option<(LcuAuth, PathBuf)> {
    let mut paths = running_league_lockfile_paths();
    paths.extend(candidate_lockfile_paths(overrides));

    let mut seen = HashSet::<String>::new();
    let mut deduped = Vec::new();
    for path in paths {
        let key = path_key(&path);
        if seen.insert(key) {
            deduped.push(path);
        }
    }

    for path in deduped {
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };

        let auth = match parse_lockfile(&contents) {
            Some(auth) => auth,
            None => {
                logger::warn(&format!(
                    "invalid lockfile format: {} ({})",
                    path.display(),
                    lockfile_debug_summary(&contents)
                ));
                continue;
            }
        };

        if let Err(err) = probe_connection(client, &auth) {
            logger::warn(&format!(
                "lockfile found but probe failed ({}): {err:?}",
                path.display()
            ));
            continue;
        }

        logger::info(&format!(
            "LCU connected (protocol={}, port={}, lockfile={})",
            auth.protocol,
            auth.port,
            path.display()
        ));
        return Some((auth, path));
    }

    None
}

fn probe_connection(client: &reqwest::blocking::Client, auth: &LcuAuth) -> anyhow::Result<()> {
    let url = format!("{}/lol-gameflow/v1/gameflow-phase", base_url(auth));
    let response = client
        .get(url)
        .basic_auth("riot", Some(&auth.password))
        .send()
        .context("probe /lol-gameflow/v1/gameflow-phase")?;

    if response.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("probe status {}", response.status())
    }
}

fn base_url(auth: &LcuAuth) -> String {
    format!("{}://127.0.0.1:{}", auth.protocol, auth.port)
}

fn trim_for_log(text: &str) -> String {
    const LIMIT: usize = 512;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    format!("{}...(truncated)", &text[..LIMIT])
}

fn build_client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .timeout(Duration::from_secs(2))
        .build()
        .context("build HTTP client")
}

fn get_gameflow_phase(
    client: &reqwest::blocking::Client,
    auth: &LcuAuth,
) -> anyhow::Result<String> {
    let url = format!("{}/lol-gameflow/v1/gameflow-phase", base_url(auth));

    let response = client
        .get(url)
        .basic_auth("riot", Some(&auth.password))
        .send()
        .context("GET /lol-gameflow/v1/gameflow-phase")?;

    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        anyhow::bail!(
            "gameflow phase HTTP status: {status}, body={}",
            trim_for_log(&body)
        );
    }

    serde_json::from_str(&body).context("parse gameflow phase")
}

fn accept_ready_check(client: &reqwest::blocking::Client, auth: &LcuAuth) -> anyhow::Result<()> {
    let url = format!("{}/lol-matchmaking/v1/ready-check/accept", base_url(auth));

    let response = client
        .post(url)
        .basic_auth("riot", Some(&auth.password))
        .send()
        .context("POST /lol-matchmaking/v1/ready-check/accept")?;

    let status = response.status();
    let body = response.text().unwrap_or_default();

    if status.is_success() {
        Ok(())
    } else {
        anyhow::bail!(
            "ready-check accept HTTP status: {status}, body={}",
            trim_for_log(&body)
        );
    }
}

fn parse_lockfile(contents: &str) -> Option<LcuAuth> {
    let mut contents = contents.trim_matches(|c: char| c.is_whitespace() || c == '\u{0}');
    contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    if contents.is_empty() {
        return None;
    }

    let parts: Vec<&str> = contents.split(':').collect();
    if parts.len() < 4 {
        return None;
    }

    let (port_str, password_str, protocol_str) = if parts.len() >= 5 {
        (
            parts[parts.len() - 3],
            parts[parts.len() - 2],
            parts[parts.len() - 1],
        )
    } else {
        (parts[1], parts[2], parts[3])
    };

    let port: u16 = port_str.trim().parse().ok()?;
    if port == 0 {
        return None;
    }

    let password = password_str.trim().to_string();
    if password.is_empty() {
        return None;
    }

    let protocol = protocol_str.trim().to_ascii_lowercase();
    if protocol != "https" && protocol != "http" {
        return None;
    }

    Some(LcuAuth {
        protocol,
        port,
        password,
    })
}

fn lockfile_debug_summary(contents: &str) -> String {
    let mut contents = contents.trim_matches(|c: char| c.is_whitespace() || c == '\u{0}');
    contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);

    let len = contents.len();
    let colons = contents.as_bytes().iter().filter(|&&b| b == b':').count();
    let parts: Vec<&str> = contents.split(':').collect();

    let first = parts.first().map(|p| p.trim()).unwrap_or("<empty>");
    let first = if first.chars().count() > 32 {
        let prefix: String = first.chars().take(32).collect();
        format!("{prefix}...")
    } else {
        first.to_string()
    };

    let last = parts
        .last()
        .map(|p| p.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let last = if last == "http" || last == "https" {
        last
    } else {
        "<redacted>".to_string()
    };

    let mut nums = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        if let Ok(v) = part.trim().parse::<u32>() {
            nums.push(format!("{idx}={v}"));
        }
    }

    format!(
        "len={len}, colons={colons}, parts={}, first=\"{first}\", last=\"{last}\", nums=[{}]",
        parts.len(),
        nums.join(",")
    )
}

fn candidate_lockfile_paths(overrides: &LcuOverrides) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(lockfile) = overrides.lockfile.clone() {
        paths.push(lockfile);
    }

    if let Some(dir) = overrides.lol_dir.clone() {
        paths.push(dir.join("lockfile"));
    }

    if let Some(lockfile) = std::env::var_os("LOL_LOCKFILE") {
        paths.push(PathBuf::from(lockfile));
    }

    if let Some(dir) = std::env::var_os("LOL_DIR") {
        paths.push(PathBuf::from(dir).join("lockfile"));
    }

    for riot_dir in riot_installed_league_dirs() {
        paths.push(riot_dir.join("lockfile"));
    }

    for base in [
        r"C:\Riot Games\League of Legends",
        r"C:\Program Files\Riot Games\League of Legends",
        r"C:\Program Files (x86)\Riot Games\League of Legends",
    ] {
        paths.push(PathBuf::from(base).join("lockfile"));
    }

    paths
}

fn running_league_lockfile_paths() -> Vec<PathBuf> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    };

    let mut results = Vec::new();

    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(snapshot) => snapshot,
            Err(_) => return results,
        };

        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let exe_name = utf16_nul_terminated_to_string(&entry.szExeFile);

            if is_league_client_process(&exe_name) {
                let pid = entry.th32ProcessID;

                if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut buffer = vec![0u16; 32 * 1024];
                    let mut len: u32 = buffer.len().try_into().unwrap_or(u32::MAX);

                    let ok = QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_FORMAT(0),
                        windows::core::PWSTR(buffer.as_mut_ptr()),
                        &mut len,
                    )
                    .is_ok();

                    if ok {
                        let path = String::from_utf16_lossy(&buffer[..len as usize]);
                        let path = PathBuf::from(path);
                        if let Some(dir) = path.parent() {
                            results.push(dir.join("lockfile"));
                        }
                    }

                    let _ = CloseHandle(process);
                }
            }

            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }

        let _ = CloseHandle(snapshot);
    }

    let mut seen = HashSet::<String>::new();
    results.retain(|path| seen.insert(path_key(path)));
    results
}

fn utf16_nul_terminated_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn is_league_client_process(exe_name: &str) -> bool {
    exe_name.eq_ignore_ascii_case("LeagueClientUx.exe")
        || exe_name.eq_ignore_ascii_case("LeagueClientUxRender.exe")
        || exe_name.eq_ignore_ascii_case("LeagueClient.exe")
}

fn riot_installed_league_dirs() -> Vec<PathBuf> {
    let Some(programdata) = std::env::var_os("PROGRAMDATA") else {
        return Vec::new();
    };

    let installs_path = PathBuf::from(programdata)
        .join("Riot Games")
        .join("RiotClientInstalls.json");

    let Ok(contents) = fs::read_to_string(installs_path) else {
        return Vec::new();
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Vec::new();
    };

    let serde_json::Value::Object(map) = json else {
        return Vec::new();
    };

    let mut results = Vec::new();
    let mut seen = HashSet::<String>::new();

    for (key, value) in map.iter() {
        if key.starts_with("league_of_legends") {
            let Some(path) = value.as_str() else {
                continue;
            };
            if let Some(path) = normalize_league_install_path(PathBuf::from(path)) {
                push_dedup(&mut results, &mut seen, path);
            }
        }
    }

    if let Some(serde_json::Value::Object(associated)) = map.get("associated_client") {
        for (install_dir, _client_path) in associated {
            if let Some(path) = normalize_league_install_path(PathBuf::from(install_dir)) {
                push_dedup(&mut results, &mut seen, path);
            }
        }
    }

    results
}

fn normalize_league_install_path(path: PathBuf) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path);
    }

    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return path.parent().map(PathBuf::from);
    }

    None
}

fn push_dedup(results: &mut Vec<PathBuf>, seen: &mut HashSet<String>, path: PathBuf) {
    let key = path_key(&path);
    if seen.insert(key) {
        results.push(path);
    }
}

fn path_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().replace('\\', "/");
    while key.ends_with('/') {
        key.pop();
    }
    key.make_ascii_lowercase();
    key
}

fn print_help() {
    println!(
        "lol_plugin (Windows)\n\nUSAGE:\n  lol_plugin.exe [--lol-dir <path>] [--lockfile <path>]\n\nOPTIONS:\n  --lol-dir <path>   League of Legends install directory (contains LeagueClient.exe)\n  --lockfile <path>  Full path to the LCU lockfile\n  -h, --help, /?     Print this help\n\nNOTES:\n  You can also set env vars LOL_DIR or LOL_LOCKFILE.\n  The lockfile exists only while the League client is running."
    );
}
