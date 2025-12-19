# lol_plugin

Win 平台托盘常驻的 LOL LCU ReadyCheck 自动接受工具（仅个人使用）。

## 运行原理

- 从本机 `lockfile`（或 `RiotClientInstalls.json` 推导的安装目录）读取 LCU 的 `port`/`token`
- 轮询 `GET /lol-gameflow/v1/gameflow-phase`
- 当进入 `ReadyCheck` 时调用 `POST /lol-matchmaking/v1/ready-check/accept`

## 配置（可选）

默认会尝试从 `C:\ProgramData\Riot Games\RiotClientInstalls.json` 和常见安装路径定位 `lockfile`。

如果你的安装路径比较特殊，可用环境变量指定：

- `LOL_LOCKFILE`：直接指定 `lockfile` 完整路径
- `LOL_DIR`：指定 LoL 安装目录（程序会自动拼接 `lockfile`）

也可以用命令行参数（更适合做快捷方式）：

- `--lockfile <path>`
- `--lol-dir <path>`

## 构建（Windows）

1. 安装 Rust（MSVC 工具链）：`rustup default stable-x86_64-pc-windows-msvc`
2. 在项目目录执行：`cargo build --release`
3. 运行：`target\release\lol_plugin.exe`

启动后会出现在系统托盘，右键菜单只有“退出”。
