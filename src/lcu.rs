use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Context;

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
    port: u16,
    password: String,
}

pub fn run(stop: Arc<AtomicBool>, overrides: LcuOverrides) {
    let client = match build_client() {
        Ok(client) => client,
        Err(_) => return,
    };

    let mut accepted_this_ready_check = false;
    let mut auth: Option<LcuAuth> = None;

    while !stop.load(Ordering::Relaxed) {
        if auth.is_none() {
            auth = read_lockfile_auth(&overrides);
            accepted_this_ready_check = false;

            if auth.is_none() {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }

        let Some(lcu) = auth.as_ref() else {
            continue;
        };

        let phase = match get_gameflow_phase(&client, lcu) {
            Ok(phase) => phase,
            Err(_) => {
                auth = None;
                accepted_this_ready_check = false;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        if phase == "ReadyCheck" {
            if !accepted_this_ready_check {
                if accept_ready_check(&client, lcu).is_ok() {
                    accepted_this_ready_check = true;
                }
            }
        } else {
            accepted_this_ready_check = false;
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn build_client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(2))
        .build()
        .context("build HTTP client")
}

fn get_gameflow_phase(
    client: &reqwest::blocking::Client,
    auth: &LcuAuth,
) -> anyhow::Result<String> {
    let url = format!(
        "https://127.0.0.1:{}/lol-gameflow/v1/gameflow-phase",
        auth.port
    );

    let response = client
        .get(url)
        .basic_auth("riot", Some(&auth.password))
        .send()
        .context("GET /lol-gameflow/v1/gameflow-phase")?;

    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        anyhow::bail!("gameflow phase HTTP status: {status}");
    }

    serde_json::from_str(&body).context("parse gameflow phase")
}

fn accept_ready_check(client: &reqwest::blocking::Client, auth: &LcuAuth) -> anyhow::Result<()> {
    let url = format!(
        "https://127.0.0.1:{}/lol-matchmaking/v1/ready-check/accept",
        auth.port
    );

    let response = client
        .post(url)
        .basic_auth("riot", Some(&auth.password))
        .send()
        .context("POST /lol-matchmaking/v1/ready-check/accept")?;

    if response.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("ready-check accept HTTP status: {}", response.status());
    }
}

fn read_lockfile_auth(overrides: &LcuOverrides) -> Option<LcuAuth> {
    for path in candidate_lockfile_paths(overrides) {
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };

        let auth = match parse_lockfile(contents.trim()) {
            Some(auth) => auth,
            None => continue,
        };

        return Some(auth);
    }
    None
}

fn parse_lockfile(contents: &str) -> Option<LcuAuth> {
    let parts: Vec<&str> = contents.split(':').collect();
    if parts.len() != 5 {
        return None;
    }

    let port = parts[2].parse().ok()?;
    let password = parts[3].to_string();

    Some(LcuAuth { port, password })
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

    if let Some(riot_dir) = riot_installed_league_dir() {
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

fn riot_installed_league_dir() -> Option<PathBuf> {
    let programdata = std::env::var_os("PROGRAMDATA")?;
    let installs_path = PathBuf::from(programdata)
        .join("Riot Games")
        .join("RiotClientInstalls.json");

    let contents = fs::read_to_string(installs_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;

    if let Some(path) = json.get("league_of_legends.live").and_then(|v| v.as_str()) {
        return normalize_league_install_path(PathBuf::from(path));
    }

    let serde_json::Value::Object(map) = json else {
        return None;
    };

    for (key, value) in map {
        if !key.starts_with("league_of_legends") {
            continue;
        }
        if let Some(path) = value.as_str() {
            if let Some(path) = normalize_league_install_path(PathBuf::from(path)) {
                return Some(path);
            }
        }
    }

    None
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

fn print_help() {
    println!(
        "lol_plugin (Windows)\n\nUSAGE:\n  lol_plugin.exe [--lol-dir <path>] [--lockfile <path>]\n\nOPTIONS:\n  --lol-dir <path>   League of Legends install directory (contains LeagueClient.exe)\n  --lockfile <path>  Full path to the LCU lockfile\n  -h, --help, /?     Print this help\n\nNOTES:\n  You can also set env vars LOL_DIR or LOL_LOCKFILE.\n  The lockfile exists only while the League client is running."
    );
}
