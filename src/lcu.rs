use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Context;

#[derive(Clone, Default)]
pub struct LcuOverrides {
    lol_dir: Option<PathBuf>,
}

impl LcuOverrides {
    pub fn from_args(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut overrides = Self::default();
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            match arg.as_str() {
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
    log_info!("lcu worker start");

    let client = match build_client() {
        Ok(client) => client,
        Err(_err) => {
            log_error!("build HTTP client failed: {_err:?}");
            return;
        }
    };

    let mut accepted_this_ready_check = false;
    let mut connection: Option<(LcuAuth, String)> = None;
    let mut last_phase: Option<String> = None;
    let mut fullmuted_this_game = false;

    while !stop.load(Ordering::Relaxed) {
        if connection.is_none() {
            connection = discover_connection(&client, &overrides);
            accepted_this_ready_check = false;
            last_phase = None;
            fullmuted_this_game = false;

            if connection.is_none() {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }

        let Some((auth, _source)) = connection.as_ref() else {
            continue;
        };

        let phase = match get_gameflow_phase(&client, auth) {
            Ok(phase) => phase,
            Err(_err) => {
                log_warn!("LCU request failed (source={_source}): {_err:?}");
                connection = None;
                accepted_this_ready_check = false;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        if last_phase.as_deref() != Some(phase.as_str()) {
            log_info!("gameflow phase: {phase}");
            last_phase = Some(phase.clone());
        }

        if phase == "InProgress" {
            if !fullmuted_this_game {
                fullmuted_this_game = true;
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let _ = crate::ingame::try_fullmute_all_after_enter_game(stop);
                });
            }
        } else {
            fullmuted_this_game = false;
        }

        if phase == "ReadyCheck" {
            if !accepted_this_ready_check {
                log_info!("ready-check: attempting accept");
                match accept_ready_check(&client, auth) {
                    Ok(()) => {
                        log_info!("ready-check: accepted");
                        accepted_this_ready_check = true;
                    }
                    Err(_err) => {
                        log_warn!("ready-check accept failed: {_err:?}");
                    }
                }
            }
        } else {
            accepted_this_ready_check = false;
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    log_info!("lcu worker stop");
}

fn discover_connection(
    client: &reqwest::blocking::Client,
    overrides: &LcuOverrides,
) -> Option<(LcuAuth, String)> {
    let filter = desired_client_dir_filter(overrides);
    for (auth, source) in running_league_auths_from_processes(filter.as_ref()) {
        if let Err(_err) = probe_connection(client, &auth) {
            log_warn!("process auth probe failed ({source}): {_err:?}");
            continue;
        }
        log_info!(
            "LCU connected via process (protocol={}, port={}, source={source})",
            auth.protocol,
            auth.port
        );
        return Some((auth, source));
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

fn utf16_nul_terminated_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn is_league_client_process(exe_name: &str) -> bool {
    exe_name.eq_ignore_ascii_case("LeagueClientUx.exe")
        || exe_name.eq_ignore_ascii_case("LeagueClientUxRender.exe")
        || exe_name.eq_ignore_ascii_case("LeagueClient.exe")
}

fn running_league_auths_from_processes(filter: Option<&ClientDirFilter>) -> Vec<(LcuAuth, String)> {
    use core::ffi::c_void;

    use windows::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::{
                Debug::ReadProcessMemory,
                ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                    TH32CS_SNAPPROCESS,
                },
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
            },
        },
    };

    use windows::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation,
    };

    const STATUS_BUFFER_TOO_SMALL: i32 = 0xC0000023u32 as i32;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;

    #[repr(C)]
    #[derive(Default)]
    struct ProcessBasicInfo {
        exit_status: i32,
        peb_base_address: *mut c_void,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    #[derive(Default)]
    struct RtlUserProcessParameters {
        reserved1: [u8; 16],
        reserved2: [*mut c_void; 10],
        _image_path_name: UnicodeString,
        command_line: UnicodeString,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Peb {
        reserved1: [u8; 2],
        being_debugged: u8,
        reserved2: u8,
        reserved3: [*mut c_void; 2],
        _ldr: *mut c_void,
        process_parameters: *mut RtlUserProcessParameters,
    }

    #[derive(Default, Clone)]
    struct PartialAuth {
        protocol: Option<String>,
        port: Option<u16>,
        password: Option<String>,
    }

    impl PartialAuth {
        fn merge(&mut self, other: PartialAuth, _key: &str) {
            if self.protocol.is_none() {
                self.protocol = other.protocol;
            } else if let (Some(a), Some(b)) = (self.protocol.as_ref(), other.protocol.as_ref()) {
                if a != b {
                    log_warn!("conflicting LCU protocol for same client dir ({_key}): {a} vs {b}");
                }
            }

            if self.port.is_none() {
                self.port = other.port;
            } else if let (Some(a), Some(b)) = (self.port, other.port) {
                if a != b {
                    log_warn!("conflicting LCU port for same client dir ({_key}): {a} vs {b}");
                }
            }

            if self.password.is_none() {
                self.password = other.password;
            } else if let (Some(a), Some(b)) = (self.password.as_ref(), other.password.as_ref()) {
                if a != b {
                    log_warn!("conflicting LCU remoting-auth-token for same client dir ({_key})");
                }
            }
        }

        fn into_auth(self) -> Option<LcuAuth> {
            let port = self.port?;
            let password = self.password?;
            let protocol = self.protocol.unwrap_or_else(|| "https".to_string());

            Some(LcuAuth {
                protocol,
                port,
                password,
            })
        }
    }

    unsafe fn read_command_line_ntquery(
        process: windows::Win32::Foundation::HANDLE,
    ) -> anyhow::Result<String> {
        let mut size_bytes = 16 * 1024;
        for _ in 0..6 {
            let mut buffer_u16 = vec![0u16; (size_bytes + 1) / 2];
            let buffer_size = (buffer_u16.len() * 2) as u32;

            let mut return_len = 0u32;
            let status = NtQueryInformationProcess(
                process,
                ProcessCommandLineInformation,
                buffer_u16.as_mut_ptr() as *mut c_void,
                buffer_size,
                &mut return_len,
            );

            if status.0 == STATUS_INFO_LENGTH_MISMATCH || status.0 == STATUS_BUFFER_TOO_SMALL {
                let suggested = return_len as usize;
                size_bytes = size_bytes
                    .max(suggested)
                    .saturating_add(4096)
                    .saturating_mul(2);
                continue;
            }

            if status.0 < 0 {
                anyhow::bail!(
                    "NtQueryInformationProcess(ProcessCommandLineInformation) failed: {status:?}"
                );
            }

            let ustr: UnicodeString =
                std::ptr::read_unaligned(buffer_u16.as_ptr() as *const UnicodeString);

            if ustr.length == 0 || ustr.buffer.is_null() {
                anyhow::bail!("command line is empty");
            }

            let base = buffer_u16.as_ptr() as usize;
            let end = base + buffer_u16.len() * 2;
            let ptr = ustr.buffer as usize;

            if ptr < base || ptr.saturating_add(ustr.length as usize) > end {
                anyhow::bail!("command line buffer points outside returned buffer");
            }

            let offset_bytes = ptr - base;
            if offset_bytes % 2 != 0 {
                anyhow::bail!("command line buffer is not u16-aligned");
            }

            let offset_u16 = offset_bytes / 2;
            let len_u16 = (ustr.length as usize) / 2;
            if offset_u16.saturating_add(len_u16) > buffer_u16.len() {
                anyhow::bail!("command line length out of bounds");
            }

            let slice = &buffer_u16[offset_u16..offset_u16 + len_u16];
            return Ok(String::from_utf16_lossy(slice));
        }

        anyhow::bail!("command line query exceeded retry budget")
    }

    unsafe fn read_command_line_peb(
        process: windows::Win32::Foundation::HANDLE,
    ) -> anyhow::Result<String> {
        let mut info = ProcessBasicInfo::default();
        let mut return_len = 0u32;

        let status = NtQueryInformationProcess(
            process,
            ProcessBasicInformation,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ProcessBasicInfo>() as u32,
            &mut return_len,
        );

        if status.0 < 0 {
            anyhow::bail!("NtQueryInformationProcess failed: {status:?}");
        }

        if info.peb_base_address.is_null() {
            anyhow::bail!("peb_base_address is null");
        }

        let mut peb = Peb::default();
        ReadProcessMemory(
            process,
            info.peb_base_address as *const c_void,
            &mut peb as *mut _ as *mut c_void,
            std::mem::size_of::<Peb>(),
            None,
        )
        .context("ReadProcessMemory(PEB)")?;

        if peb.process_parameters.is_null() {
            anyhow::bail!("process_parameters is null");
        }

        let mut params = RtlUserProcessParameters::default();
        ReadProcessMemory(
            process,
            peb.process_parameters as *const c_void,
            &mut params as *mut _ as *mut c_void,
            std::mem::size_of::<RtlUserProcessParameters>(),
            None,
        )
        .context("ReadProcessMemory(ProcessParameters)")?;

        let cmd = params.command_line;
        if cmd.length == 0 || cmd.buffer.is_null() {
            anyhow::bail!("command line is empty");
        }

        let len_u16 = (cmd.length as usize) / 2;
        let mut buffer = vec![0u16; len_u16];
        ReadProcessMemory(
            process,
            cmd.buffer as *const c_void,
            buffer.as_mut_ptr() as *mut c_void,
            cmd.length as usize,
            None,
        )
        .context("ReadProcessMemory(CommandLine)")?;

        Ok(String::from_utf16_lossy(&buffer))
    }

    fn find_cmd_arg_value(cmd: &str, name: &str) -> Option<String> {
        let bytes = cmd.as_bytes();
        let mut start = 0usize;
        let mut best: Option<String> = None;

        while start < cmd.len() {
            let Some(rel) = cmd[start..].find(name) else {
                break;
            };
            let idx = start + rel;

            if idx > 0 {
                let prev = bytes[idx - 1];
                if !prev.is_ascii_whitespace() && prev != b'"' && prev != b'\'' {
                    start = idx + name.len();
                    continue;
                }
            }

            let mut j = idx + name.len();
            if bytes.get(j) == Some(&b'=') {
                j += 1;
            } else {
                while bytes.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
                    j += 1;
                }
            }

            if j >= cmd.len() {
                start = idx + name.len();
                continue;
            }

            let quote = bytes[j];
            let (val, end) = if quote == b'"' || quote == b'\'' {
                j += 1;
                let mut k = j;
                while k < cmd.len() && bytes[k] != quote {
                    k += 1;
                }
                (&cmd[j..k], k.saturating_add(1))
            } else {
                let mut k = j;
                while k < cmd.len() {
                    let b = bytes[k];
                    if b.is_ascii_whitespace() || b == b'"' || b == b'\'' {
                        break;
                    }
                    k += 1;
                }
                (&cmd[j..k], k)
            };

            if !val.is_empty() {
                best = Some(val.to_string());
            }
            start = end.max(idx + name.len());
        }

        best
    }

    fn parse_partial_auth_from_command_line(cmd: &str) -> PartialAuth {
        let protocol = find_cmd_arg_value(cmd, "--app-protocol")
            .map(|s| s.trim_matches('"').to_ascii_lowercase())
            .filter(|s| s == "http" || s == "https");

        let port = find_cmd_arg_value(cmd, "--app-port")
            .and_then(|s| s.trim_matches('"').parse::<u16>().ok())
            .filter(|&p| p != 0);

        let password = find_cmd_arg_value(cmd, "--remoting-auth-token")
            .map(|s| s.trim_matches('"').to_string())
            .filter(|s| !s.is_empty());

        PartialAuth {
            protocol,
            port,
            password,
        }
    }

    struct Group {
        dir: PathBuf,
        partial: PartialAuth,
        sources: Vec<String>,
    }

    let mut groups: HashMap<String, Group> = HashMap::new();

    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(snapshot) => snapshot,
            Err(_) => return Vec::new(),
        };

        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let exe_name = utf16_nul_terminated_to_string(&entry.szExeFile);
            if !is_league_client_process(&exe_name) {
                ok = Process32NextW(snapshot, &mut entry).is_ok();
                continue;
            }

            let pid = entry.th32ProcessID;

            let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(process) => process,
                Err(_err) => {
                    log_warn!("OpenProcess failed (pid={pid}, exe={exe_name}): {_err:?}");
                    ok = Process32NextW(snapshot, &mut entry).is_ok();
                    continue;
                }
            };

            let mut exe_buf = vec![0u16; 32 * 1024];
            let mut exe_len: u32 = exe_buf.len().try_into().unwrap_or(u32::MAX);
            let exe_path = if QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(exe_buf.as_mut_ptr()),
                &mut exe_len,
            )
            .is_ok()
            {
                Some(PathBuf::from(String::from_utf16_lossy(
                    &exe_buf[..exe_len as usize],
                )))
            } else {
                None
            };

            let Some(exe_path) = exe_path else {
                let _ = CloseHandle(process);
                ok = Process32NextW(snapshot, &mut entry).is_ok();
                continue;
            };

            let Some(exe_dir) = exe_path.parent() else {
                let _ = CloseHandle(process);
                ok = Process32NextW(snapshot, &mut entry).is_ok();
                continue;
            };

            let client_dir_key = path_key(exe_dir);
            let source_base = format!("pid={pid}, exe_name={exe_name}, exe={}", exe_path.display());

            let (cmd, cmd_source) = match read_command_line_ntquery(process) {
                Ok(cmd) => (Some(cmd), "cmdline_ntq"),
                Err(_err) => {
                    log_warn!("failed to read command line (NtQuery) ({source_base}): {_err:?}");
                    (None, "cmdline_ntq")
                }
            };

            if let Some(cmd) = cmd {
                let partial = parse_partial_auth_from_command_line(&cmd);
                let has_any = partial.port.is_some() || partial.password.is_some();
                if has_any {
                    let entry = groups
                        .entry(client_dir_key.clone())
                        .or_insert_with(|| Group {
                            dir: exe_dir.to_path_buf(),
                            partial: PartialAuth::default(),
                            sources: Vec::new(),
                        });
                    entry
                        .sources
                        .push(format!("{source_base}, via={cmd_source}"));
                    entry.partial.merge(partial, &client_dir_key);
                } else {
                    log_warn!("process command line has no LCU args ({source_base})");
                }
            } else {
                let process_vm = match OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                    false,
                    pid,
                ) {
                    Ok(process) => process,
                    Err(_err) => {
                        log_warn!("OpenProcess(PROCESS_VM_READ) failed ({source_base}): {_err:?}");
                        let _ = CloseHandle(process);
                        ok = Process32NextW(snapshot, &mut entry).is_ok();
                        continue;
                    }
                };

                match read_command_line_peb(process_vm) {
                    Ok(cmd) => {
                        let partial = parse_partial_auth_from_command_line(&cmd);
                        let has_any = partial.port.is_some() || partial.password.is_some();
                        if has_any {
                            let entry =
                                groups
                                    .entry(client_dir_key.clone())
                                    .or_insert_with(|| Group {
                                        dir: exe_dir.to_path_buf(),
                                        partial: PartialAuth::default(),
                                        sources: Vec::new(),
                                    });
                            entry
                                .sources
                                .push(format!("{source_base}, via=cmdline_peb"));
                            entry.partial.merge(partial, &client_dir_key);
                        } else {
                            log_warn!("process command line has no LCU args ({source_base})");
                        }
                    }
                    Err(_err) => {
                        log_warn!("failed to read command line (PEB) ({source_base}): {_err:?}");
                    }
                }

                let _ = CloseHandle(process_vm);
            }

            let _ = CloseHandle(process);
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }

        let _ = CloseHandle(snapshot);
    }

    let mut results = Vec::new();
    for (key, group) in groups {
        if let Some(filter) = filter {
            if !filter.matches_key(&key) {
                continue;
            }
        }

        let partial = group.partial.clone();
        let _has_port = partial.port.is_some();
        let _has_token = partial.password.is_some();

        if let Some(auth) = partial.into_auth() {
            let sources = if group.sources.is_empty() {
                "unknown".to_string()
            } else {
                group.sources.join(" | ")
            };
            results.push((
                auth,
                format!("client_dir={}, sources={}", group.dir.display(), sources),
            ));
        } else {
            log_warn!(
                "found partial LCU auth but not enough to connect (client_dir={}, port={}, token={})",
                group.dir.display(),
                if _has_port { "yes" } else { "no" },
                if _has_token { "yes" } else { "no" }
            );
        }
    }

    results
}

fn path_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().replace('\\', "/");
    while key.ends_with('/') {
        key.pop();
    }
    key.make_ascii_lowercase();
    key
}

#[derive(Clone)]
struct ClientDirFilter {
    exact: String,
    prefix: String,
}

impl ClientDirFilter {
    fn new(path: &Path) -> Self {
        let exact = path_key(path);
        let prefix = format!("{exact}/");
        Self { exact, prefix }
    }

    fn matches_key(&self, key: &str) -> bool {
        key == self.exact || key.starts_with(&self.prefix)
    }
}

fn desired_client_dir_filter(overrides: &LcuOverrides) -> Option<ClientDirFilter> {
    if let Some(dir) = overrides.lol_dir.as_deref() {
        return Some(ClientDirFilter::new(dir));
    }

    if let Some(dir) = std::env::var_os("LOL_DIR") {
        return Some(ClientDirFilter::new(Path::new(&dir)));
    }

    None
}

fn print_help() {
    println!(
        "lol_plugin (Windows)\n\nUSAGE:\n  lol_plugin.exe [--lol-dir <path>]\n\nOPTIONS:\n  --lol-dir <path>   Filter which running client to use (useful when multiple clients are running)\n  -h, --help, /?     Print this help\n\nNOTES:\n  This program connects to LCU only via running League client processes.\n  If the client is not running, it retries every 1 second.\n  You can also set env var LOL_DIR."
    );
}
