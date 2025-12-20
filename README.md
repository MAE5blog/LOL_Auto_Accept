# lol_plugin

Win 平台托盘常驻的 LOL LCU ReadyCheck 自动接受工具（仅个人使用）。

## 运行原理

- 从正在运行的 LOL 客户端进程命令行读取 LCU 的 `port`/`token`（`--app-port` / `--remoting-auth-token`）
- 轮询 `GET /lol-gameflow/v1/gameflow-phase`
- 当进入 `ReadyCheck` 时调用 `POST /lol-matchmaking/v1/ready-check/accept`

## 正式版说明

- 托盘常驻，右键菜单只有“退出”
- 默认不输出日志、不生成日志文件
- 仅在客户端运行后才能连接 LCU；如果未检测到客户端，会每 1 秒重试一次
- 为了向游戏窗口发送键盘输入（例如自动 `/fullmute all`），启动时会请求管理员权限（UAC）

## 配置（可选）

如果你电脑上同时运行了多个服/多个客户端，可用参数或环境变量“指定要监听的那个目录”：

- `LOL_DIR`：过滤目标客户端目录（用于多开时指定）
- `--lol-dir <path>`：同上（更适合做快捷方式）

## 构建（Windows）

1. 安装 Rust（MSVC 工具链）：`rustup default stable-x86_64-pc-windows-msvc`
2. 在项目目录执行：`cargo build --release`
3. 运行：`target\release\lol_plugin.exe`

启动后会出现在系统托盘，右键菜单只有“退出”。

## 发布 Releases（自动）

仓库已配置 GitHub Actions：推送 tag 后会自动编译并把 exe 附到 Releases。

示例：

- `git tag v0.1.0`
- `git push origin v0.1.0`
