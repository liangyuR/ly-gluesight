# 独立生产模拟器

`tools/production-simulator` 是独立 Rust 命令行工具。它提供本地 S7 PLC 和 TCP 相机图像流，GlueSight 按生产通讯协议 8 接件、采集、调用 lyFlow 测量、判定、录制和回结果。工具不调用应用的运行 IPC，也不提供模拟测量结论。

工具只监听 `127.0.0.1`。PLC 使用现有 S7 客户端和协议，相机新增 `simulator` 来源；海康设备与真实 PLC 的驱动、通讯协议和配置不变。联调必须启动独立资料目录的隔离实例，不能把现场正在运行的实例改成模拟设备。

## 构建与启动

在仓库根目录运行（Rust 版本以 `rust-toolchain.toml` 为准）：

```powershell
cargo build --release --locked --manifest-path tools/production-simulator/Cargo.toml
```

可执行文件为 `tools/production-simulator/target/release/gluesight-production-simulator.exe`。工具不依赖 Tauri、海康 SDK 或 lyFlow DLL；执行测量的 GlueSight 仍需可用的 lyFlow 运行库。

首次验证端口和图像传输，可生成三台灰度相机的条图样本：

```powershell
tools/production-simulator/target/release/gluesight-production-simulator.exe --init output/simulator-demo
tools/production-simulator/target/release/gluesight-production-simulator.exe --config output/simulator-demo/simulator.json --check
tools/production-simulator/target/release/gluesight-production-simulator.exe --config output/simulator-demo/simulator.json
```

`--init` 拒绝覆盖已有目录。演示图用于通路检查，不会自动生成正式配方，也不保证判定 OK。要模拟正常生产，使用下面的现场原图及匹配的正式配方。

## 准备正常生产输入

1. 从完整生产录制件中选择一件已知结果的样本，包含 `part.json` 和全部相机原图。
2. 在隔离项目中导入与该件匹配的配方、相机 ID 和标定。相机几何、图像模式、尺寸、走廊、分段与正式发布包必须匹配；不要只修改型号让无关配方接件。
3. 转换原图并生成独立工具配置：

```powershell
python -m pip install -r tools/production-simulator/requirements.txt
python scripts/Prepare-ProductionSimulation.py `
  --record "<包含 part.json 的录制件目录>" `
  --output output/simulator-production `
  --product-code 1 --frame-rate 10 --pass-frames 30,20,40
```

参数中的型号、帧率和每笔帧数需填写实际值；每笔帧数合计必须等于该件帧组数。只有单笔时可不传 `--pass-frames`。脚本只读取源资料，在新目录中生成 PNG；带 Bayer 注释的原图转为彩色 PNG，并输出每台相机需要的 `imageMode`。支持普通 PNM、gzip 和带校验和的 zstd 原图，保留 Bayer 相位并拒绝损坏或超过 128 MiB 的输入。依赖 Python、numpy、OpenCV（`opencv-python-headless`）；zstd 依赖通过 `python -m pip install -r scripts/requirements-images.txt` 安装。相机地址、帧序列、笔间隔等保存在 `simulator.json`，路径相对于该文件。

多笔之间按 `passGapMs` 拉低门控；`captureLeadMs` 不小于 2000 ms，模拟机械臂通知到拍照开始的间隔。`frameRate` 是工具发图频率，需与隔离实例相机设置一致。建议使用 release 工具；大图解码或接收端背压会影响实际节拍，应在应用中核对实测帧率和缺帧提示。

## 配置隔离的 GlueSight

先安装仓库依赖（`pnpm install --frozen-lockfile`），然后使用 [测试说明](TESTING.md) 中的隔离实例入口：

```powershell
pwsh -NoProfile -File scripts/Build-UiTestInstance.ps1 -Profile flow
pwsh -NoProfile -File scripts/Start-UiTestInstance.ps1 -Profile flow -SkipBuild
```

脚本使用独立应用标识和 `output/playwright/flow-regression` 下的测试资料，不复用现场实例配置。不要传入现场资料目录。测试结束时按实例清单核对身份并关闭：

```powershell
& ./scripts/Stop-UiTestInstance.ps1 -Profile flow `
  -Instance (Get-Content output/playwright/flow-regression/instance.json -Raw)
```

仅在隔离项目内操作：

| 项目 | 配置 |
| --- | --- |
| PLC | 协议 8，S7 地址 `127.0.0.1`，端口 `1102`，默认 PLC 区 DB4 / PC 区 DB2，rack 0 / slot 1；点表采用现有协议模板 |
| 相机 | 来源“外部模拟器”，ID 必须与配方及配置一致，地址为工具输出的 `127.0.0.1:12001` 等，每台单视角 |
| 生产采集 | `Line0`，默认 10 fps，与工具一致；灰度/彩色模式以转换脚本输出为准 |
| 单帧调试 | 可临时选 `Software` 并点击软触发，工具返回首帧；生产前恢复 `Line0` |
| 配方 | 对应型号有可投产的正式发布包，真实 lyFlow 就绪，相机标定与原图匹配 |
| 模式 | 正常生产验收关闭维护模式；维护模式仍会按协议回 0，不能当作检测 OK |

先启动工具，再连接隔离实例 PLC 和相机。确认三个相机就绪、PLC 心跳正常、协议版本为 8；需要时执行软件复位，双方回空闲/就绪。工具不会改写 GlueSight 的配置文件、连接现场 PLC 或连接海康设备。

## 运行与验收

交互控制台支持：

```text
run 1     运行一件
run 10    连续十件
run 0     持续运行
stop      当前件完成后停止
reset     PLC 发起复位，中断在途件
quit      退出工具，断开模拟连接
```

也可批量执行：

```powershell
tools/production-simulator/target/release/gluesight-production-simulator.exe `
  --config output/simulator-production/simulator.json --cycles 10
```

启动运行时最多等待 60 秒确认 PLC 和所有相机连接。每件使用递增的请求序号和 `SIM-0000000001` 形式的新 SN，复用配置的原图序列。正常状态依次为：

1. PLC 写型号、序号、SN，最后置 `plcState=10`。
2. 等待配置的机械臂提前量，再开启门控并按固定频率发图；不等待 PC 对接件请求的确认。
3. 全部笔结束，门控拉低，置 `plcState=20`。
4. PC 真实测量并回 `pcState=30`；工具连续核对稳定的序号、SN、结果码和异常码，成功才置 `plcState=30`。
5. PC 回 `pcState=10` 后，PLC 回 `plcState=0`，进入下一件。

每件打印一行 JSON，含 `sequence`、`sn`、`result`、`fault`、`maintenance`。批次中未检、通讯失败或结果身份不符，会在批次结束时报错并以非零状态退出；合法 NG/检测错误是已完成的握手，退出码不代表检测 OK。正常样本应得到预期的 1/2 或 NG；检测错误 90 表示配方、相机或测量仍有问题，不能当作正常生产验收完成。请同时核对应用里的型号/SN、三相机帧组与笔数、真实测量结果、历史记录和原图。模拟输入会按正常生产来源落库，必须使用隔离资料和显式的 `SIM` SN 前缀。

工件到达时 PC 未就绪则记未检、不提交请求；NG 不停线。心跳失活、结果身份不符等协议问题进入 PLC 故障，允许复位恢复。`stop` 在本件完成后停止，`quit`/Ctrl+C 会直接断开，不能替代完整收尾。

## 网络相机契约

每台相机一个 TCP 端口，只接受一个客户端。每包为 4 字节大端 JSON 长度、UTF-8 JSON 头、像素字节。头字段为 `width`、`height`、`frameNumber`、`timestampNs`、`pixelFormat`（`mono8` / `rgb8`）、`payloadLength`、`gateHigh`。像素按行紧排，RGB 顺序，不含压缩和行填充。帧号跨工件递增，三相机同一帧组使用同一曝光时间戳。

控制包的宽、高、像素长度为 0，约每 20 ms 发送一次，并在门控变化时立即发送；只更新连接、计数基线和门控，不计入帧数。软件触发由客户端发送单行 `{"command":"trigger"}`，空闲时返回首帧。断连后相机适配自动重连；帧头最多 4096 字节，单帧最多 64 MiB，尺寸/像素长度不符时拒绝输入。

工具模拟的是 S7 和图像数据链路，不模拟海康网络协议、电气信号、曝光响应或真实机器人运动。多机硬件时序、海康丢包、现场节拍仍需真实设备验证。自动故障注入和持续吞吐判定尚无新工具入口。
